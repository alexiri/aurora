//! `aurora template …` (docs/25, C-1105): offline tooling for template publishers. No server,
//! no login. All validation lives in `aurora-template`; this module only collects files, prints
//! and exits.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use aurora_core::limits::TEMPLATES_PER_SOURCE_MAX;
use aurora_template::{
    TreeFile, content_sha256, pack, parse_manifest, parse_meta, read_archive, read_tree,
    render_package_url,
};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum TemplateCmd {
    /// Validate a template directory, a packed zip, or a repository folder (has
    /// `aurora-templates.yaml`).
    Check {
        /// A template directory, a `.zip` package, or a repository folder.
        path: PathBuf,
    },
    /// Validate a template directory and write its deterministic zip.
    Pack {
        /// The template's directory (named by its id).
        dir: PathBuf,
        /// Output file (default: `<id>-<version>.zip` in the current directory).
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    /// List a repository folder's templates (what CI would publish).
    List {
        /// The repository folder (has `aurora-templates.yaml`).
        path: PathBuf,
        /// Print a JSON array instead of lines.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(cmd: TemplateCmd) -> Result<i32> {
    match cmd {
        TemplateCmd::Check { path } => {
            let out = check(&path)?;
            for line in &out.problems {
                println!("{line}");
            }
            for line in &out.ok {
                println!("{line}");
            }
            Ok(i32::from(!out.problems.is_empty()))
        }
        TemplateCmd::Pack { dir, out } => {
            let r = pack_dir(&dir, out.as_deref())?;
            println!(
                "{} sha256:{} content:{} {} bytes",
                r.file.display(),
                r.archive_sha256,
                r.content_sha256,
                r.size
            );
            Ok(0)
        }
        TemplateCmd::List { path, json } => {
            let items = list(&path)?;
            if json {
                let v: Vec<_> = items
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.id, "version": t.version, "name": t.name, "dir": t.dir,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for t in items {
                    println!("{}\t{}\t{}\t{}", t.id, t.version, t.name, t.dir);
                }
            }
            Ok(0)
        }
    }
}

/// One `check` result: problem lines (any → exit 1) and `ok …` lines.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CheckOut {
    pub problems: Vec<String>,
    pub ok: Vec<String>,
}

/// C-1105 `check`: a template directory, a packed zip, or a repository folder.
pub fn check(path: &Path) -> Result<CheckOut> {
    if path.is_file() {
        let bytes =
            std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
        return Ok(match read_archive(&bytes) {
            Ok(c) => CheckOut {
                problems: Vec::new(),
                ok: vec![ok_line(&c)],
            },
            Err(problems) => CheckOut {
                problems: problems.iter().map(|p| p.to_string()).collect(),
                ok: Vec::new(),
            },
        });
    }
    if !path.is_dir() {
        bail!("{} is neither a file nor a directory", path.display());
    }
    if path.join("aurora-templates.yaml").is_file() {
        return check_repo(path);
    }
    let (root, files) = collect_template(path)?;
    Ok(match read_tree(&root, &files) {
        Ok(c) => CheckOut {
            problems: Vec::new(),
            ok: vec![ok_line(&c)],
        },
        Err(problems) => CheckOut {
            problems: problems.iter().map(|p| p.to_string()).collect(),
            ok: Vec::new(),
        },
    })
}

/// `ok <id> <version> sha256:<content_sha256>` (C-1105).
fn ok_line(c: &aurora_template::TemplateContent) -> String {
    format!(
        "ok {} {} sha256:{}",
        c.meta.id,
        c.meta.version,
        content_sha256(c)
    )
}

/// C-1105 `check` on a repository folder: the manifest, every template directory, each
/// directory named by its id, and a `packages.url` that renders for every template.
fn check_repo(folder: &Path) -> Result<CheckOut> {
    let manifest_bytes = std::fs::read(folder.join("aurora-templates.yaml"))?;
    let mut out = CheckOut::default();
    let manifest = match parse_manifest(&manifest_bytes) {
        Ok(m) => m,
        Err(p) => {
            out.problems.push(format!("aurora-templates.yaml: {p}"));
            return Ok(out);
        }
    };
    for dir in template_dirs(folder)? {
        let (root, files) = collect_template(&folder.join(&dir))?;
        match read_tree(&root, &files) {
            Ok(c) => match render_package_url(&manifest, &c.meta.id, &c.meta.version) {
                Ok(_) => out.ok.push(ok_line(&c)),
                Err(p) => out.problems.push(format!("{}: {p}", c.meta.id)),
            },
            Err(problems) => out.problems.extend(problems.iter().map(|p| p.to_string())),
        }
    }
    Ok(out)
}

/// C-1105 `pack`: validate, write the deterministic zip, report both hashes and the size.
fn pack_dir(dir: &Path, out: Option<&Path>) -> Result<PackOut> {
    let (root, files) = collect_template(dir)?;
    let content = read_tree(&root, &files).map_err(TemplateErrors)?;
    let zip = pack(&root, &files);
    let file = match out {
        Some(p) => p.to_owned(),
        None => PathBuf::from(format!("{}-{}.zip", content.meta.id, content.meta.version)),
    };
    std::fs::write(&file, &zip).with_context(|| format!("could not write {}", file.display()))?;
    Ok(PackOut {
        file,
        archive_sha256: sha256_hex(&zip),
        content_sha256: content_sha256(&content),
        size: zip.len(),
    })
}

#[derive(Debug)]
pub struct PackOut {
    pub file: PathBuf,
    pub archive_sha256: String,
    pub content_sha256: String,
    pub size: usize,
}

/// A template with problems, as data (`Err` of `read_tree`), surfaced as an anyhow error whose
/// message is the problem lines.
#[derive(Debug)]
struct TemplateErrors(Vec<aurora_template::TemplateProblem>);

impl std::fmt::Display for TemplateErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for p in &self.0 {
            writeln!(f, "{p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TemplateErrors {}

/// One row of `list` (C-1105).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateInfo {
    pub id: String,
    pub version: String,
    pub name: String,
    pub dir: String,
}

/// C-1105 `list`: the manifest, then `template.yaml` alone per template directory (the ingest's
/// listing read, C-1104).
fn list(folder: &Path) -> Result<Vec<TemplateInfo>> {
    let manifest_bytes = std::fs::read(folder.join("aurora-templates.yaml"))?;
    if let Err(p) = parse_manifest(&manifest_bytes) {
        bail!("aurora-templates.yaml: {p}");
    }
    let mut out = Vec::new();
    for dir in template_dirs(folder)? {
        let bytes = std::fs::read(folder.join(&dir).join("template.yaml"))?;
        let meta = parse_meta(&bytes).map_err(TemplateErrors)?;
        out.push(TemplateInfo {
            id: meta.id,
            version: meta.version,
            name: meta.name,
            dir,
        });
    }
    Ok(out)
}

/// The template directories directly in `folder`: every directory containing `template.yaml`,
/// sorted (C-1103). At most `TEMPLATES_PER_SOURCE_MAX` per source folder (C-1103).
fn template_dirs(folder: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if entry.path().join("template.yaml").is_file() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("a directory name is not valid UTF-8"))?;
            out.push(name);
        }
    }
    out.sort();
    if out.len() > TEMPLATES_PER_SOURCE_MAX {
        bail!(
            "{} holds {} template directories, more than the {} per source (TEMPLATES_PER_SOURCE_MAX)",
            folder.display(),
            out.len(),
            TEMPLATES_PER_SOURCE_MAX
        );
    }
    Ok(out)
}

/// Reads one template directory as a git tree: files under `<root>/…`, the executable bit as the
/// mode. Symlinks are refused (C-1141 accepts regular files only).
fn collect_template(dir: &Path) -> Result<(String, Vec<TreeFile>)> {
    let root = dir
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no directory name", dir.display()))?
        .to_owned();
    let mut files = Vec::new();
    walk(dir, &root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((root, files))
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<TreeFile>) -> Result<()> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .with_context(|| format!("could not read {}", dir.display()))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let md = e
            .metadata()
            .with_context(|| format!("could not stat {}", e.path().display()))?;
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("a file name is not valid UTF-8"))?;
        let path = format!("{prefix}/{name}");
        if md.is_dir() {
            // A `.github` folder is a repository's own metadata, never template content: not
            // walked, not packed (aurora_core::meta_paths).
            if name == ".github" {
                continue;
            }
            walk(&e.path(), &path, out)?;
        } else {
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                if md.mode() & 0o111 != 0 {
                    0o100755
                } else {
                    0o100644
                }
            };
            #[cfg(not(unix))]
            let mode = 0o100644;
            // A symlink's `metadata` follows it, but `symlink_metadata` on the entry itself
            // does not; `DirEntry::file_type` never follows.
            if e.file_type()?.is_symlink() {
                bail!("{} is a symlink: templates accept regular files only", path);
            }
            out.push(TreeFile {
                path,
                bytes: std::fs::read(e.path())?,
                mode,
            });
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    use std::fs;

    fn tmp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("aurora-tpl-{name}-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    const TEMPLATE_YAML: &str = "\
format: 1
id: demo-agent
version: 1.2.0
name: Demo
summary: A demo template
";

    const README: &str = "# Demo\n\nWhat this agent is for.\n";

    const AGENT_YAML: &str = "name: Demo agent\n";

    const INSTRUCTIONS: &str = "Do the demo things.\n";

    /// A minimal valid template directory (the id in `template.yaml` is the directory's name).
    fn write_template(root: &Path, id: &str) {
        let d = root.join(id);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("template.yaml"),
            TEMPLATE_YAML.replace("demo-agent", id),
        )
        .unwrap();
        fs::write(d.join("README.md"), README).unwrap();
        fs::write(d.join("agent.yaml"), AGENT_YAML).unwrap();
        fs::write(d.join("instructions.md"), INSTRUCTIONS).unwrap();
    }

    const MANIFEST: &str = "\
api: 1
repo:
  name: Demo templates
  description: Templates for testing
packages:
  url: https://example.com/templates/{id}/{version}/{id}-{version}.zip
";

    fn write_repo(root: &Path) {
        fs::write(root.join("aurora-templates.yaml"), MANIFEST).unwrap();
        fs::write(
            root.join("README.md"),
            "# The repository's own README; ignored\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(".ci")).unwrap();
        fs::write(root.join(".ci/ci.yml"), "ci: true\n").unwrap();
    }

    #[test]
    fn check_template_directory_is_ok() {
        let root = tmp_dir("dir");
        write_template(&root, "demo-agent");
        let out = check(&root.join("demo-agent")).unwrap();
        assert!(out.problems.is_empty());
        assert_eq!(out.ok.len(), 1);
        assert!(out.ok[0].starts_with("ok demo-agent 1.2.0 sha256:"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_directory_reports_problems_and_lines() {
        let root = tmp_dir("broken");
        // The directory name does not match the id in template.yaml, the README is missing and
        // notes.txt is not part of the format.
        let d = root.join("wrong-name");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("template.yaml"), TEMPLATE_YAML).unwrap();
        fs::write(d.join("notes.txt"), "x\n").unwrap();
        let out = check(&d).unwrap();
        assert!(out.ok.is_empty());
        assert!(out.problems.iter().any(|p| p.contains("template_dir_name")));
        assert!(
            out.problems
                .iter()
                .any(|p| p.contains("template_readme_missing"))
        );
        assert!(
            out.problems
                .iter()
                .any(|p| p.contains("template_file_unexpected"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_zip_matches_the_directory() {
        let root = tmp_dir("zip");
        write_template(&root, "demo-agent");
        let dir = root.join("demo-agent");
        let packed = pack_dir(&dir, Some(&root.join("out.zip"))).unwrap();
        assert_eq!(packed.file, root.join("out.zip"));
        assert_eq!(packed.size, std::fs::read(&packed.file).unwrap().len());
        let from_dir = check(&dir).unwrap();
        let from_zip = check(&root.join("out.zip")).unwrap();
        assert_eq!(from_dir, from_zip);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pack_default_name_and_deterministic_bytes() {
        // The default output name is relative to the process's working directory; tests run in
        // parallel, so packs that use it are serialized.
        static PACK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let root = tmp_dir("pack");
        write_template(&root, "demo-agent");
        let dir = root.join("demo-agent");
        let _g = PACK_LOCK.lock().unwrap();
        let a = pack_dir(&dir, None).unwrap();
        // Default: `<id>-<version>.zip` in the current directory.
        assert_eq!(a.file, PathBuf::from("demo-agent-1.2.0.zip"));
        let first = fs::read(&a.file).unwrap();
        let b = pack_dir(&dir, None).unwrap();
        assert_eq!(a.archive_sha256, b.archive_sha256);
        assert_eq!(first, fs::read(&b.file).unwrap());
        // Both packs use the same default name: one file to clean up.
        std::fs::remove_file(&a.file).unwrap();
        drop(_g);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pack_refuses_a_broken_template() {
        let root = tmp_dir("packbad");
        let d = root.join("demo-agent");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("template.yaml"), TEMPLATE_YAML).unwrap();
        let e = pack_dir(&d, None).unwrap_err();
        assert!(e.to_string().contains("template_readme_missing"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_and_list_repository_folder() {
        let root = tmp_dir("repo");
        write_repo(&root);
        write_template(&root, "demo-agent");
        write_template(&root, "other-agent");
        // A directory without template.yaml and a loose file are ignored (C-1103).
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("notes.txt"), "ignored\n").unwrap();
        let out = check(&root).unwrap();
        assert!(out.problems.is_empty(), "{:?}", out.problems);
        assert_eq!(out.ok.len(), 2);
        assert!(out.ok.iter().all(|l| l.starts_with("ok ")));

        let items = list(&root).unwrap();
        assert_eq!(
            items,
            vec![
                TemplateInfo {
                    id: "demo-agent".into(),
                    version: "1.2.0".into(),
                    name: "Demo".into(),
                    dir: "demo-agent".into(),
                },
                TemplateInfo {
                    id: "other-agent".into(),
                    version: "1.2.0".into(),
                    name: "Demo".into(),
                    dir: "other-agent".into(),
                },
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_repository_reports_a_bad_manifest() {
        let root = tmp_dir("manifest");
        fs::write(root.join("aurora-templates.yaml"), "api: 2\n").unwrap();
        let out = check(&root).unwrap();
        assert!(out.ok.is_empty());
        assert_eq!(out.problems.len(), 1);
        assert!(out.problems[0].starts_with("aurora-templates.yaml: template_manifest_invalid"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn check_repository_reports_a_bad_template_with_its_problems() {
        let root = tmp_dir("repobad");
        write_repo(&root);
        let d = root.join("wrong-name");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("template.yaml"), TEMPLATE_YAML).unwrap();
        let out = check(&root).unwrap();
        assert!(out.ok.is_empty());
        assert!(out.problems.iter().any(|p| p.contains("template_dir_name")));
        assert!(
            out.problems
                .iter()
                .any(|p| p.contains("template_readme_missing"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn more_templates_than_the_per_source_cap_are_refused() {
        let root = tmp_dir("cap");
        for i in 0..TEMPLATES_PER_SOURCE_MAX + 1 {
            let d = root.join(format!("t{i:03}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("template.yaml"), "").unwrap();
        }
        let err = template_dirs(&root).unwrap_err().to_string();
        assert!(err.contains("more than the 100 per source"), "{err}");
        // At the cap exactly, the folder is still readable.
        fs::remove_dir_all(root.join("t100")).unwrap();
        assert_eq!(
            template_dirs(&root).unwrap().len(),
            TEMPLATES_PER_SOURCE_MAX
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn list_refuses_a_bad_template_yaml() {
        let root = tmp_dir("listbad");
        fs::write(root.join("aurora-templates.yaml"), MANIFEST).unwrap();
        let d = root.join("demo-agent");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("template.yaml"),
            "format: 2\nid: demo-agent\nversion: 1.0.0\nname: N\nsummary: S\n",
        )
        .unwrap();
        let e = list(&root).unwrap_err();
        assert!(
            e.to_string().contains("template_format_unsupported"),
            "got: {e}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn non_existing_path_is_an_error() {
        let root = tmp_dir("missing");
        assert!(check(&root.join("nope")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
