//! The packed form (C-1141): [`read_archive`] parses the zip itself — central directory, stored
//! and deflate through `flate2` — and refuses anything dangerous before inflating: encryption,
//! other methods, Zip64, names and modes that fail the shared checks, declared sizes that lie,
//! and the 100× ratio. [`pack`] writes the deterministic zip the CLI and CI publish (C-1144).
//!
//! The reader is written for template packages only: every file must live under one top-level
//! directory named by the template's id, exactly like the web export's zip (`export/zip.ts`),
//! which this module must interop with in both directions.

use std::io::Read;

use crc32fast::Hasher as Crc;
use flate2::read::DeflateDecoder;

use aurora_core::limits;

use crate::model::TreeFile;
use crate::problem::{ProblemCode, TemplateProblem};
use crate::validate::{mode_problem, read_files, rel_path_problem};

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
/// The UTF-8 name flag both writers set.
const FLAG_UTF8: u16 = 0x0800;

/// One structural zip refusal (C-1141's `template_archive`).
fn bad(message: impl Into<String>) -> Vec<TemplateProblem> {
    vec![TemplateProblem::new(
        ProblemCode::TemplateArchive,
        "<archive>",
        message,
    )]
}

/// The packed size cap, checked before anything else (C-1141).
fn too_large() -> Vec<TemplateProblem> {
    vec![TemplateProblem::new(
        ProblemCode::TemplateTooLarge,
        "<archive>",
        format!(
            "the packed template is larger than {} bytes",
            limits::TEMPLATE_ARCHIVE_MAX
        ),
    )]
}

/// Reads a packed template (C-1104): the zip is parsed defensively, the inflated files go
/// through the same validation as the tree form.
pub fn read_archive(bytes: &[u8]) -> Result<crate::model::TemplateContent, Vec<TemplateProblem>> {
    if bytes.len() > limits::TEMPLATE_ARCHIVE_MAX {
        return Err(too_large());
    }
    // The end-of-central-directory record: the last 22..(22 + 65535) bytes, scanned backwards.
    if bytes.len() < 22 {
        return Err(bad("this is not a zip file"));
    }
    let mut eocd = None;
    let scan_from = bytes.len().saturating_sub(22 + 0xffff);
    let mut i = bytes.len() - 22;
    loop {
        if u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) == EOCD_SIG {
            eocd = Some(i);
            break;
        }
        if i == scan_from {
            break;
        }
        i -= 1;
    }
    let Some(eocd) = eocd else {
        return Err(bad("this is not a zip file"));
    };
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let count = u16_at(eocd + 10) as usize;
    let cd_size = u32_at(eocd + 12) as usize;
    let cd_offset = u32_at(eocd + 16) as usize;
    // Zip64 values (0xFFFF / 0xFFFFFFFF) are refused, as the web reader refuses them.
    if count == 0xffff
        || cd_size == 0xffff_ffff
        || cd_offset == 0xffff_ffff
        || u16_at(eocd + 8) == 0xffff
    {
        return Err(bad("Zip64 archives are not supported"));
    }
    if eocd + 22 + u16_at(eocd + 20) as usize > bytes.len() {
        return Err(bad("the zip is damaged"));
    }
    let mut problems = Vec::new();
    let mut files: Vec<TreeFile> = Vec::new();
    let mut unpacked = 0usize;
    let mut root: Option<String> = None;
    let mut pos = cd_offset;
    for _ in 0..count {
        if pos + 46 > bytes.len() || u32_at(pos) != CENTRAL_SIG {
            return Err(bad("the zip is damaged"));
        }
        let flags = u16_at(pos + 8);
        let method = u16_at(pos + 10);
        let crc = u32_at(pos + 16);
        let csize = u32_at(pos + 20) as usize;
        let usize_ = u32_at(pos + 24) as usize;
        let nlen = u16_at(pos + 28) as usize;
        let xlen = u16_at(pos + 30) as usize;
        let clen = u16_at(pos + 32) as usize;
        let attrs = u32_at(pos + 38);
        let local = u32_at(pos + 42) as usize;
        if pos + 46 + nlen > bytes.len() {
            return Err(bad("the zip is damaged"));
        }
        let Ok(name) = std::str::from_utf8(&bytes[pos + 46..pos + 46 + nlen]) else {
            return Err(bad("a file name in the zip is not valid UTF-8 text"));
        };
        pos += 46 + nlen + xlen + clen;
        // Directory entries are ignored (C-1141): the trailing slash, or a Unix dir mode.
        if name.ends_with('/') || (attrs >> 16) & 0o170000 == 0o040000 {
            continue;
        }
        if flags & 1 == 1 {
            return Err(bad("the zip is encrypted"));
        }
        if flags & !(FLAG_UTF8 | 0b1000) != 0 {
            return Err(bad("the zip uses flags this reader does not accept"));
        }
        match method {
            METHOD_STORED | METHOD_DEFLATE => {}
            _ => {
                return Err(bad(
                    "the zip uses a compression this reader does not accept",
                ));
            }
        }
        // Names and modes before anything is inflated (SEC-146).
        if let Some(p) = rel_path_problem(name) {
            problems.push(p);
            continue;
        }
        if let Some(p) = root_problem(name, &mut root) {
            problems.push(p);
            continue;
        }
        // The declared sizes are checked before inflation: a 100× ratio is refused (no floor),
        // and a size that exceeds the unpacked cap is refused without allocating it.
        if usize_ > 100 * csize {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateTooLarge,
                name,
                "the entry unpacks to more than 100 times its packed size",
            ));
            continue;
        }
        unpacked += usize_;
        if unpacked > limits::TEMPLATE_UNPACKED_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateTooLarge,
                name,
                format!(
                    "the unpacked template is larger than {} bytes",
                    limits::TEMPLATE_UNPACKED_MAX
                ),
            ));
            break;
        }
        if files.len() >= limits::TEMPLATE_FILES_MAX {
            problems.push(TemplateProblem::new(
                ProblemCode::TemplateTooLarge,
                name,
                format!(
                    "the template has more than {} files",
                    limits::TEMPLATE_FILES_MAX
                ),
            ));
            break;
        }
        // The local header: only its signature, name length and extra length are trusted for
        // finding the data; the sizes come from the central directory.
        if local + 30 > bytes.len() || u32_at(local) != LOCAL_SIG {
            return Err(bad("the zip is damaged"));
        }
        let start = local + 30 + u16_at(local + 26) as usize + u16_at(local + 28) as usize;
        // Checked: a hostile central directory may name a local header (and lengths) that run
        // past the end of the archive; `bytes.len() - start` would underflow (SEC-146).
        let Some(rest) = bytes.len().checked_sub(start) else {
            return Err(bad("the zip is damaged"));
        };
        if csize > rest {
            return Err(bad("the zip is damaged"));
        }
        let raw = &bytes[start..start + csize];
        let data = match inflate(method, raw, usize_, crc) {
            Ok(d) => d,
            Err(m) => return Err(bad(format!("{name}: {m}"))),
        };
        // The zip gives the Unix mode in the high bytes of the external attributes; the shared
        // mode check refuses anything but a regular file. Attributes of 0 (the web export's
        // writer records none) are a plain, non-executable file.
        let high = attrs >> 16;
        let mode = match high & 0o170000 {
            0o100000 => {
                if high & 0o100 != 0 {
                    0o100755
                } else {
                    0o100644
                }
            }
            0 => 0o100644,
            other => other, // symlink, submodule, device: refused by the shared check
        };
        if let Some(p) = mode_problem(name, mode) {
            problems.push(p);
            continue;
        }
        files.push(TreeFile {
            path: name.to_owned(),
            bytes: data,
            mode,
        });
    }
    if !problems.is_empty() {
        return Err(crate::problem::finish_problems(problems));
    }
    let Some(root) = root else {
        return Err(bad("the zip holds no files"));
    };
    read_files(&root, &files)
}

/// The one top-level directory: every entry must live under the same `<root>/`, which is the
/// template directory (C-1141). Returns a `template_dir_name` problem for anything else.
fn root_problem(name: &str, root: &mut Option<String>) -> Option<TemplateProblem> {
    let top = name.split('/').next().unwrap_or(name);
    match root {
        None => {
            *root = Some(top.to_owned());
            None
        }
        Some(r) if r == top => None,
        Some(_) => Some(TemplateProblem::new(
            ProblemCode::TemplateDirName,
            name,
            "every file must live under the one template directory at the top of the zip",
        )),
    }
}

/// Inflates one entry, verifying the declared size and the CRC. `declared` was already checked
/// against the ratio and the caps, so the allocation is bounded by it.
fn inflate(method: u16, raw: &[u8], declared: usize, crc: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(declared.min(1 << 20));
    match method {
        METHOD_STORED => out.extend_from_slice(raw),
        METHOD_DEFLATE => {
            let mut decoder = DeflateDecoder::new(raw).take((declared as u64) + 1);
            decoder
                .read_to_end(&mut out)
                .map_err(|_| "the compressed data is damaged".to_owned())?;
        }
        _ => return Err("unsupported compression method".into()),
    }
    if out.len() != declared {
        return Err("the declared size does not match the data".into());
    }
    let mut hasher = Crc::new();
    hasher.update(&out);
    if hasher.finalize() != crc {
        return Err("the entry is damaged (checksum mismatch)".into());
    }
    Ok(out)
}

/// `pack` (C-1104, C-1105): the deterministic template zip — sorted names, the fixed DOS
/// timestamp 1980-01-01, stored entries, the UTF-8 flag and the executable bit in the external
/// attributes, exactly what the web export's writer and this reader agree on. CI compares a
/// fresh pack with a published package byte for byte, so nothing here may depend on time, order
/// or the environment.
///
/// The names must have passed [`crate::read_tree`]; they are re-checked and a bad one is a
/// caller bug, refused with a panic (the API and the CLI always validate first).
pub fn pack(root_name: &str, files: &[TreeFile]) -> Vec<u8> {
    let mut files: Vec<&TreeFile> = files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out: Vec<u8> = Vec::new();
    // (crc, size, external attributes, local offset) per file, in pack order.
    let mut central: Vec<(u32, u32, u32, u32)> = Vec::new();
    for f in &files {
        let Some(rel) = f
            .path
            .strip_prefix(root_name)
            .and_then(|r| r.strip_prefix('/'))
        else {
            refuse_pack(&format!("{}: not under the root directory", f.path));
        };
        if let Some(p) = rel_path_problem(rel) {
            refuse_pack(&format!("{}: {}", p.path, p.message));
        }
        if f.path.len() <= root_name.len() + 1 {
            refuse_pack(&format!("{}: not under the root directory", f.path));
        }
        if mode_problem(&f.path, f.mode).is_some() {
            refuse_pack(&format!("{}: bad mode {:#o}", f.path, f.mode));
        }
        let name = f.path.as_bytes();
        let crc = {
            let mut h = Crc::new();
            h.update(&f.bytes);
            h.finalize()
        };
        let size = f.bytes.len() as u32;
        let attrs = f.mode << 16;
        let offset = out.len() as u32;
        central.push((crc, size, attrs, offset));
        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // DOS time: midnight
        out.extend_from_slice(&0x0021u16.to_le_bytes()); // DOS date: 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name);
        out.extend_from_slice(&f.bytes);
    }
    let cd_offset = out.len() as u32;
    for ((crc, size, attrs, offset), f) in central.iter().zip(&files) {
        let name = f.path.as_bytes();
        out.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // DOS time
        out.extend_from_slice(&0x0021u16.to_le_bytes()); // DOS date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        out.extend_from_slice(&attrs.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name);
    }
    let cd_size = (out.len() as u32) - cd_offset;
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with the directory
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

/// Refuse a bad name in `pack`: unreachable for validated input, a bug otherwise.
#[allow(clippy::panic)] // a defensive refusal, never reachable from read_tree-validated files
fn refuse_pack(message: &str) -> ! {
    panic!("aurora_template::pack: {message}");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::hash::content_sha256;
    use crate::model::TemplateContent;
    use crate::test_fixtures::{TEMPLATE_YAML, sample_files};
    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use std::io::Write;

    fn tree_file(path: &str, bytes: &[u8], mode: u32) -> TreeFile {
        TreeFile {
            path: path.to_owned(),
            bytes: bytes.to_vec(),
            mode,
        }
    }

    /// A stored zip like the web export's writer: UTF-8 flag, no Unix attributes, the current
    /// DOS time (which read_archive must not care about).
    fn stored_zip(entries: &[(&str, &[u8], u32)], attrs_override: Option<u32>) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<(String, u32, u32, u32, u32)> = Vec::new();
        for (name, data, mode) in entries {
            let crc = {
                let mut h = Crc::new();
                h.update(data);
                h.finalize()
            };
            let attrs = attrs_override.unwrap_or_else(|| mode << 16);
            let offset = out.len() as u32;
            central.push(((*name).to_owned(), crc, data.len() as u32, offset, attrs));
            out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            out.extend_from_slice(&METHOD_STORED.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0x0021u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
        }
        let cd_offset = out.len() as u32;
        for (name, crc, size, offset, attrs) in &central {
            out.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            out.extend_from_slice(&METHOD_STORED.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0x0021u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&attrs.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
        }
        let cd_size = out.len() as u32 - cd_offset;
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(central.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// A deflate zip: one entry compressed, the sizes declared correctly.
    fn deflate_zip(name: &str, data: &[u8], declared_lying: bool) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        let compressed = encoder.finish().unwrap();
        let crc = {
            let mut h = Crc::new();
            h.update(data);
            h.finalize()
        };
        let declared = if declared_lying {
            data.len() + 1
        } else {
            data.len()
        };
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0021u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(declared as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&compressed);
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0021u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(declared as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(0o100644u32 << 16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        let cd_size = out.len() as u32 - cd_offset;
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn two_files_zip() -> Vec<u8> {
        stored_zip(
            &[
                ("t/README.md", b"# T\n".as_slice(), 0o100644),
                (
                    "t/template.yaml",
                    TEMPLATE_YAML.replace("service-monitor", "t").as_bytes(),
                    0o100644,
                ),
                ("t/agent.yaml", b"format: 1\nname: T\n".as_slice(), 0o100644),
                ("t/instructions.md", b"Do the thing.\n".as_slice(), 0o100644),
            ],
            None,
        )
    }

    #[test]
    fn pack_roundtrips_and_agrees_with_read_tree() {
        // read_tree and read_archive produce the same content from the same files (C-1104,
        // C-1119): the hashes must be equal, and the fields too.
        let files = sample_files();
        let from_tree: TemplateContent = crate::read_tree("service-monitor", &files).unwrap();
        let packed = pack("service-monitor", &files);
        let from_zip = crate::read_archive(&packed).unwrap();
        assert_eq!(content_sha256(&from_tree), content_sha256(&from_zip));
        assert_eq!(from_tree.instructions, from_zip.instructions);
        assert_eq!(from_tree.skills, from_zip.skills);
        assert_eq!(from_tree.helpers, from_zip.helpers);
        assert_eq!(from_tree.memories, from_zip.memories);
        assert_eq!(from_tree.automations, from_zip.automations);
        assert_eq!(from_tree.agent_name, from_zip.agent_name);
    }

    #[test]
    fn pack_is_deterministic() {
        let files = sample_files();
        let a = pack("service-monitor", &files);
        let mut shuffled = files.clone();
        shuffled.reverse();
        let b = pack("service-monitor", &shuffled);
        assert_eq!(a, b);
    }

    #[test]
    fn pack_refuses_bad_names() {
        let files = vec![tree_file("other/x.txt", b"x", 0o100644)];
        let result = std::panic::catch_unwind(|| pack("t", &files));
        assert!(result.is_err(), "packing a file outside the root is a bug");
    }

    #[test]
    fn web_written_zip_is_read_back() {
        // The web export's writer records no Unix attributes and uses the current DOS time; a
        // Save-as-template zip must open (C-1119).
        let bytes = stored_zip(
            &[
                ("t/README.md", b"# T\n".as_slice(), 0o100644),
                (
                    "t/template.yaml",
                    TEMPLATE_YAML.replace("service-monitor", "t").as_bytes(),
                    0o100644,
                ),
                ("t/agent.yaml", b"format: 1\nname: T\n".as_slice(), 0o100644),
                ("t/instructions.md", b"Do the thing.\n".as_slice(), 0o100644),
            ],
            Some(0),
        );
        let c = read_archive(&bytes).unwrap();
        assert_eq!(c.meta.id, "t");
        assert!(c.skills.is_empty());
    }

    #[test]
    fn deflated_zip_is_read_back() {
        // Every entry deflated, sizes declared correctly: the reader inflates (the web reader
        // does the same for re-packed zips).
        let files = sample_files();
        let mut bytes: Vec<u8> = Vec::new();
        let mut central: Vec<(String, u32, u32, u32, u32, u32)> = Vec::new();
        for f in &files {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&f.bytes).unwrap();
            let compressed = encoder.finish().unwrap();
            let crc = {
                let mut h = Crc::new();
                h.update(&f.bytes);
                h.finalize()
            };
            let offset = bytes.len() as u32;
            central.push((
                f.path.clone(),
                crc,
                f.bytes.len() as u32,
                compressed.len() as u32,
                offset,
                f.mode << 16,
            ));
            bytes.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            bytes.extend_from_slice(&20u16.to_le_bytes());
            bytes.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            bytes.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0x0021u16.to_le_bytes());
            bytes.extend_from_slice(&crc.to_le_bytes());
            bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(f.bytes.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(f.path.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(f.path.as_bytes());
            bytes.extend_from_slice(&compressed);
        }
        let cd_offset = bytes.len() as u32;
        for (name, crc, usize_, csize, offset, attrs) in &central {
            bytes.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            bytes.extend_from_slice(&20u16.to_le_bytes());
            bytes.extend_from_slice(&20u16.to_le_bytes());
            bytes.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            bytes.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0x0021u16.to_le_bytes());
            bytes.extend_from_slice(&crc.to_le_bytes());
            bytes.extend_from_slice(&csize.to_le_bytes());
            bytes.extend_from_slice(&usize_.to_le_bytes());
            bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&attrs.to_le_bytes());
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
        }
        let cd_size = bytes.len() as u32 - cd_offset;
        bytes.extend_from_slice(&EOCD_SIG.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&cd_size.to_le_bytes());
        bytes.extend_from_slice(&cd_offset.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        let c = read_archive(&bytes).unwrap();
        assert_eq!(c.meta.id, "service-monitor");
        assert_eq!(c.skills.len(), 1);
    }

    #[test]
    fn ratio_and_lying_sizes_are_refused() {
        // A 100× bomb: the declared unpacked size is refused before anything is inflated.
        let bytes = two_files_zip();
        // Sanity: the honest archive reads.
        assert!(read_archive(&bytes).is_ok());
        // Now a lying central directory: one entry declares 1000 bytes packed into 5.
        let bomb = lying_zip("t/README.md", b"# T\n", 1000, 5);
        let err = read_archive(&bomb).unwrap_err();
        assert!(err
            .iter()
            .any(|p| p.code == ProblemCode::TemplateTooLarge && p.message.contains("100 times")));
        // A declared size the data does not have.
        let lying = deflate_zip("t/README.md", b"# T\n", true);
        let err = read_archive(&lying).unwrap_err();
        assert!(
            err.iter()
                .any(|p| p.message.contains("declared size does not match")
                    || p.message.contains("declared"))
        );
    }

    /// A stored zip whose central directory lies about the sizes.
    fn lying_zip(name: &str, data: &[u8], declared: usize, compressed_decl: usize) -> Vec<u8> {
        let crc = {
            let mut h = Crc::new();
            h.update(data);
            h.finalize()
        };
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0021u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        out.extend_from_slice(&METHOD_STORED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0021u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(compressed_decl as u32).to_le_bytes());
        out.extend_from_slice(&(declared as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(0o100644u32 << 16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        let cd_size = out.len() as u32 - cd_offset;
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// A hostile central directory that points at a valid local header whose declared name and
    /// extra lengths run past the end of the archive: `start` must not underflow
    /// `bytes.len() - start` (SEC-146, panic on a hostile zip).
    #[test]
    fn a_local_header_past_the_end_is_damaged_not_a_panic() {
        let bytes = two_files_zip();
        assert!(read_archive(&bytes).is_ok());
        // The first local header is intact; only its name/extra lengths lie (each 0xFFFF), so
        // `start` lands far past the end of the archive.
        let mut hostile = bytes.clone();
        let local = hostile
            .windows(4)
            .position(|w| w == LOCAL_SIG.to_le_bytes())
            .unwrap();
        hostile[local + 26..local + 28].copy_from_slice(&0xffffu16.to_le_bytes());
        hostile[local + 28..local + 30].copy_from_slice(&0xffffu16.to_le_bytes());
        let err = read_archive(&hostile).unwrap_err();
        assert!(err.iter().any(|p| p.message == "the zip is damaged"));
        // The same shape with the packed size inflated: the data must not reach past the end
        // either (`csize` larger than the bytes after the local header's name).
        let mut short = bytes.clone();
        let pos = short
            .windows(4)
            .rposition(|w| w == CENTRAL_SIG.to_le_bytes())
            .unwrap();
        short[pos + 20..pos + 24].copy_from_slice(&(0xffff_ffffu32).to_le_bytes());
        let err = read_archive(&short).unwrap_err();
        assert!(err.iter().any(|p| p.message == "the zip is damaged"));
    }

    #[test]
    fn hostile_archives_are_refused() {
        // Not a zip.
        assert!(read_archive(b"this is not a zip at all........").is_err());
        // Encrypted: the general-purpose bit 0 of the central directory entry.
        let mut encrypted = two_files_zip();
        let pos = encrypted
            .windows(4)
            .rposition(|w| w == CENTRAL_SIG.to_le_bytes())
            .unwrap();
        encrypted[pos + 8] = 1;
        let err = read_archive(&encrypted).unwrap_err();
        assert!(err.iter().any(|p| p.message.contains("encrypted")));
        // A symlink entry.
        let link = stored_zip(&[("t/link", b"t/README.md".as_slice(), 0o120000)], None);
        let err = read_archive(&link).unwrap_err();
        assert!(err.iter().any(|p| p.message.contains("symlink")));
        // Two top-level directories.
        let mixed = stored_zip(
            &[
                ("t/README.md", b"# T\n".as_slice(), 0o100644),
                ("other/x.txt", b"x".as_slice(), 0o100644),
            ],
            None,
        );
        let err = read_archive(&mixed).unwrap_err();
        assert!(err.iter().any(|p| p.code == ProblemCode::TemplateDirName));
        // A name climbing out.
        let escape = stored_zip(&[("t/../escape.txt", b"x".as_slice(), 0o100644)], None);
        let err = read_archive(&escape).unwrap_err();
        assert!(err.iter().any(|p| p.code == ProblemCode::TemplateArchive));
        // Directory entries are ignored.
        let with_dir = stored_zip(
            &[
                ("t/", b"".as_slice(), 0o040755 | (0o040000u32 << 16)),
                ("t/README.md", b"# T\n".as_slice(), 0o100644),
                (
                    "t/template.yaml",
                    TEMPLATE_YAML.replace("service-monitor", "t").as_bytes(),
                    0o100644,
                ),
                ("t/agent.yaml", b"format: 1\nname: T\n".as_slice(), 0o100644),
                ("t/instructions.md", b"Do the thing.\n".as_slice(), 0o100644),
            ],
            None,
        );
        assert!(read_archive(&with_dir).is_ok());
        // An over-cap archive.
        let big = vec![b'x'; limits::TEMPLATE_ARCHIVE_MAX + 1];
        assert_eq!(
            read_archive(&big).unwrap_err()[0].code,
            ProblemCode::TemplateTooLarge
        );
    }
}
