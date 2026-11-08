//! Hook guest of the `google-workspace` built-in package: `approval.summarize` for the Drive app.
//!
//! When a Drive request is held, the gateway hands this hook the tool, method, path, query, safe
//! headers and a prefix of the body. The hook returns a short summary of what the request does:
//!
//! - uploads, metadata changes and copies: file name, target MIME type, parent folder ids and size,
//!   read from the JSON body, or from the JSON metadata part of a `multipart/related` upload;
//! - `alt=media` downloads and exports: the file id and, for exports, the export MIME type;
//! - permission changes: role, type and the email address or domain.
//!
//! The summary never contains file content: the media part of a multipart upload and the body of a
//! media upload are never read. The hook has no egress (`approval.summarize` never does) and does not
//! call `fetch`. A body it cannot read is an `invalid-input` error, and the gateway then falls back
//! to the manifest's `approvals` template.
use aurora_plugin_sdk::*;

/// Longest value the hook puts in a field (the core clamps again to its own limit).
const VALUE_MAX: usize = 120;
/// Most parent folder ids listed.
const PARENTS_MAX: usize = 5;

struct Guest;

impl Hooks for Guest {
    fn approval_summarize(i: SummarizeInput) -> Result<Summary, HookError> {
        summarize(&i)
    }
}

export_hooks!(Guest);

/// What the hook reads from the request, independent of the SDK types (native tests use this).
struct Req<'a> {
    tool: &'a str,
    path: &'a str,
    query: &'a [Kv],
    headers: &'a [Kv],
    body: &'a [u8],
    truncated: bool,
}

fn summarize(i: &SummarizeInput) -> Result<Summary, HookError> {
    let tool = i.tool_id.as_deref().ok_or(HookError::Unsupported)?;
    let req = Req {
        tool,
        path: &i.path,
        query: &i.query,
        headers: &i.headers,
        body: &i.body_prefix,
        truncated: i.truncated,
    };
    let (title, fields, text) = build(&req)?;
    Ok(Summary {
        title,
        fields: fields
            .into_iter()
            .take(12)
            .map(|(k, v)| Kv { key: k, value: v })
            .collect(),
        text,
        link: None,
        group: None,
    })
}

type Built = (String, Vec<(String, String)>, Option<String>);

fn build(r: &Req) -> Result<Built, HookError> {
    match r.tool {
        "get_file" => {
            if kv_get(r.query, "alt") != Some("media") {
                // A metadata read is not a download; nothing to summarize.
                return Err(HookError::Unsupported);
            }
            let id = file_id(r.path).ok_or_else(|| bad("no file id in the path"))?;
            Ok((
                "Download Drive file".into(),
                vec![f("File id", &id), f("Action", "download the file content")],
                None,
            ))
        }
        "export_file" => {
            let id = file_id(r.path).ok_or_else(|| bad("no file id in the path"))?;
            let mime = kv_get(r.query, "mimeType").ok_or_else(|| bad("export without mimeType"))?;
            Ok((
                "Export Drive file".into(),
                vec![f("File id", &id), f("Export MIME type", mime)],
                None,
            ))
        }
        "create_permission" => permission(r),
        "create_file" | "update_file" | "copy_file" | "upload_file" | "upload_file_content" => {
            file_change(r)
        }
        _ => Err(HookError::Unsupported),
    }
}

fn permission(r: &Req) -> Result<Built, HookError> {
    let id = file_id(r.path).ok_or_else(|| bad("no file id in the path"))?;
    let m = parse_metadata(r.body).ok_or_else(|| bad("request body is not JSON"))?;
    let (role, kind) = (m.get("role"), m.get("type"));
    if role.is_none() && kind.is_none() {
        return Err(bad("permission body has no role or type"));
    }
    let mut fields = vec![f("File id", &id)];
    if let Some(v) = role {
        fields.push(f("Role", v));
    }
    if let Some(v) = kind {
        fields.push(f("Type", v));
    }
    if let Some(v) = m.get("emailAddress") {
        fields.push(f("Email", v));
    }
    if let Some(v) = m.get("domain") {
        fields.push(f("Domain", v));
    }
    if kv_get(r.query, "transferOwnership") == Some("true") {
        fields.push(f("Ownership", "transferred to the grantee"));
    }
    if let Some(v) = kv_get(r.query, "sendNotificationEmail") {
        fields.push(f("Notify by email", v));
    }
    Ok(("Share Drive file".into(), fields, truncated_note(r)))
}

fn file_change(r: &Req) -> Result<Built, HookError> {
    let (title, upload) = match r.tool {
        "create_file" => ("Create Drive file or folder", false),
        "update_file" => ("Change Drive file", false),
        "copy_file" => ("Copy Drive file", false),
        "upload_file" => ("Upload Drive file", true),
        _ => ("Replace Drive file content", true),
    };
    let upload_type = kv_get(r.query, "uploadType").unwrap_or("");
    let ctype = hdr(r.headers, "content-type").unwrap_or("");

    // Where the JSON metadata is: the whole body, or the first part of a multipart upload. A media
    // upload has none and its body is content, which is never read.
    let meta: Option<Meta> = if upload && upload_type == "media" {
        None
    } else if upload && upload_type == "multipart" {
        let part = multipart_first_part(ctype, r.body)
            .ok_or_else(|| bad("multipart body has no readable JSON metadata part"))?;
        Some(parse_metadata(part).ok_or_else(|| bad("metadata part is not JSON"))?)
    } else if r.body.iter().all(u8::is_ascii_whitespace) {
        // PATCH with query parameters only (addParents, removeParents) is legitimate.
        Some(Meta::default())
    } else {
        Some(parse_metadata(r.body).ok_or_else(|| bad("request body is not JSON"))?)
    };

    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(id) = file_id(r.path) {
        fields.push(f("File id", &id));
    }
    if let Some(m) = &meta {
        if let Some(v) = m.get("name") {
            fields.push(f("Name", v));
        }
        if let Some(v) = m.get("mimeType") {
            fields.push(f("Target MIME type", v));
        }
        if !m.parents.is_empty() {
            fields.push(("Parent folder ids".into(), join_ids(&m.parents)));
        }
        if m.get("trashed") == Some("true") {
            fields.push(f("Trashed", "yes"));
        }
    }
    if let Some(v) = kv_get(r.query, "addParents") {
        fields.push(f("Add to folder ids", v));
    }
    if let Some(v) = kv_get(r.query, "removeParents") {
        fields.push(f("Remove from folder ids", v));
    }
    if upload {
        fields.push(f(
            "Upload type",
            if upload_type.is_empty() { "unspecified" } else { upload_type },
        ));
    }
    if upload && upload_type == "media" {
        if !ctype.is_empty() {
            fields.push(f("Content type", ctype));
        }
        if let Some(n) = hdr(r.headers, "content-length").and_then(parse_len) {
            fields.push(f("Size", &format!("{n} bytes")));
        }
    } else if let Some(n) = hdr(r.headers, "x-upload-content-length").and_then(parse_len) {
        // Resumable uploads announce the size of the content to come.
        fields.push(f("Size", &format!("{n} bytes")));
        if let Some(t) = hdr(r.headers, "x-upload-content-type") {
            fields.push(f("Content type", t));
        }
    } else if upload {
        if let Some(n) = hdr(r.headers, "content-length").and_then(parse_len) {
            fields.push(f("Request size", &format!("{n} bytes")));
        }
    }

    let has_substance = fields.iter().any(|(k, _)| k != "Upload type" && k != "File id");
    if !has_substance && !upload {
        return Err(bad("no readable metadata in the request"));
    }
    Ok((title.into(), fields, truncated_note(r)))
}

fn truncated_note(r: &Req) -> Option<String> {
    r.truncated
        .then(|| "Only the beginning of the request body was read.".to_string())
}

fn bad(m: &str) -> HookError {
    HookError::InvalidInput(m.into())
}

fn f(k: &str, v: &str) -> (String, String) {
    (k.to_string(), clean(v))
}

/// One line, no control characters, at most `VALUE_MAX` characters.
fn clean(s: &str) -> String {
    let one: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let t = one.trim();
    if t.chars().count() > VALUE_MAX {
        let cut: String = t.chars().take(VALUE_MAX - 1).collect();
        format!("{cut}…")
    } else {
        t.to_string()
    }
}

fn join_ids(ids: &[String]) -> String {
    let shown: Vec<&str> = ids.iter().take(PARENTS_MAX).map(String::as_str).collect();
    let mut out = shown.join(", ");
    if ids.len() > PARENTS_MAX {
        out.push_str(&format!(" (+{} more)", ids.len() - PARENTS_MAX));
    }
    out
}

fn hdr<'a>(list: &'a [Kv], name: &str) -> Option<&'a str> {
    list.iter()
        .find(|e| e.key.eq_ignore_ascii_case(name))
        .map(|e| e.value.as_str())
}

fn parse_len(v: &str) -> Option<u64> {
    v.trim().parse().ok()
}

/// The id after the `files` segment of `/drive/v3/files/{id}[/…]` and `/upload/drive/v3/files/{id}`.
fn file_id(path: &str) -> Option<String> {
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    segs.find(|s| *s == "files")?;
    segs.next().map(str::to_string)
}

// ---- tolerant JSON reader --------------------------------------------------------------------

/// The depth-1 members of a JSON object that the summaries use: scalar values as text, and the
/// `parents` array of strings. Only values that are complete are returned, so a body prefix cut
/// in the middle of a string never yields half a name.
#[derive(Default, Debug)]
struct Meta {
    scalars: Vec<(String, String)>,
    parents: Vec<String>,
}

impl Meta {
    fn get(&self, k: &str) -> Option<&str> {
        self.scalars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

struct Cur<'a> {
    s: &'a [u8],
    p: usize,
}

impl Cur<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.p).copied()
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.p += 1;
        }
    }
    /// A complete JSON string at the cursor, decoded; `None` if it is cut off or malformed.
    fn string(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') {
            return None;
        }
        self.p += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let b = self.peek()?;
            self.p += 1;
            match b {
                b'"' => return Some(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek()?;
                    self.p += 1;
                    match e {
                        b'n' | b'r' | b't' | b'b' | b'f' => out.push(b' '),
                        b'u' => {
                            let hex = self.s.get(self.p..self.p + 4)?;
                            self.p += 4;
                            let code = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                            let ch = char::from_u32(code).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }
    /// Skip a whole `{…}` or `[…]` at the cursor; `false` if the input ends first.
    fn skip_nested(&mut self) -> bool {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                None => return false,
                Some(b'"') => {
                    if self.string().is_none() {
                        return false;
                    }
                }
                Some(b'[' | b'{') => {
                    depth += 1;
                    self.p += 1;
                }
                Some(b']' | b'}') => {
                    self.p += 1;
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                Some(_) => self.p += 1,
            }
        }
    }
}

/// `None` when the input is not a JSON object (does not start with `{`); otherwise whatever
/// complete members precede the end of the input or the first malformed byte.
fn parse_metadata(body: &[u8]) -> Option<Meta> {
    let mut c = Cur { s: body, p: 0 };
    c.ws();
    if c.peek() != Some(b'{') {
        return None;
    }
    c.p += 1;
    let mut m = Meta::default();
    loop {
        c.ws();
        match c.peek() {
            Some(b',') => {
                c.p += 1;
                continue;
            }
            Some(b'"') => {}
            _ => break,
        }
        let Some(key) = c.string() else { break };
        c.ws();
        if c.peek() != Some(b':') {
            break;
        }
        c.p += 1;
        c.ws();
        match c.peek() {
            None => break,
            Some(b'"') => match c.string() {
                Some(v) => m.scalars.push((key, v)),
                None => break,
            },
            Some(b'[') if key == "parents" => {
                c.p += 1;
                loop {
                    c.ws();
                    match c.peek() {
                        Some(b'"') => match c.string() {
                            Some(v) => m.parents.push(v),
                            None => return Some(m),
                        },
                        Some(b',') => c.p += 1,
                        Some(b']') => {
                            c.p += 1;
                            break;
                        }
                        _ => return Some(m),
                    }
                }
            }
            Some(b'[' | b'{') => {
                if !c.skip_nested() {
                    break;
                }
            }
            Some(_) => {
                let start = c.p;
                while c
                    .peek()
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    c.p += 1;
                }
                if c.peek().is_none() {
                    break; // cut off in the middle of a literal
                }
                m.scalars
                    .push((key, String::from_utf8_lossy(&c.s[start..c.p]).into_owned()));
            }
        }
    }
    Some(m)
}

// ---- multipart/related -----------------------------------------------------------------------

/// The body of the first part of a `multipart/related` request when that part is JSON (Drive's
/// metadata part). Nothing after that part (the media) is returned or read.
fn multipart_first_part<'a>(content_type: &str, body: &'a [u8]) -> Option<&'a [u8]> {
    let lower = content_type.to_ascii_lowercase();
    let at = lower.find("boundary=")? + "boundary=".len();
    let raw = &content_type[at..];
    let boundary = match raw.strip_prefix('"') {
        Some(q) => q.split('"').next()?,
        None => raw.split([';', ' ']).next()?,
    };
    if boundary.is_empty() {
        return None;
    }
    let delim = format!("--{boundary}");
    let start = find(body, delim.as_bytes(), 0)? + delim.len();
    // Part headers end at the first blank line.
    let (hdr_end, sep_len) = match (find(body, b"\r\n\r\n", start), find(body, b"\n\n", start)) {
        (Some(a), Some(b)) if b < a => (b, 2),
        (Some(a), _) => (a, 4),
        (None, Some(b)) => (b, 2),
        (None, None) => return None,
    };
    let headers = String::from_utf8_lossy(&body[start..hdr_end]).to_ascii_lowercase();
    if !headers.contains("json") {
        return None;
    }
    let part_start = hdr_end + sep_len;
    let end_crlf = find(body, format!("\r\n{delim}").as_bytes(), part_start);
    let end_lf = find(body, format!("\n{delim}").as_bytes(), part_start);
    let end = match (end_crlf, end_lf) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => body.len(),
    };
    body.get(part_start..end)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() || from > hay.len() - needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(
        tool: &'a str,
        path: &'a str,
        query: &'a [Kv],
        headers: &'a [Kv],
        body: &'a [u8],
        truncated: bool,
    ) -> Req<'a> {
        Req { tool, path, query, headers, body, truncated }
    }

    fn field<'a>(b: &'a Built, k: &str) -> Option<&'a str> {
        b.1.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn parses_top_level_members_and_skips_nested() {
        let m = parse_metadata(
            br#"{"name":"a \"q\" ","appProperties":{"name":"nested","x":[1,{"name":"deep"}]},"mimeType":"text/plain","parents":["p1","p2"],"trashed":true}"#,
        )
        .unwrap();
        assert_eq!(m.get("name"), Some("a \"q\" "));
        // A \u escape (a backslash byte followed by u00e9) is decoded.
        let body = [&b"{\"name\":\"x"[..], &[92u8], b"u00e9\"}"].concat();
        assert_eq!(parse_metadata(&body).unwrap().get("name"), Some("x\u{e9}"));
        assert_eq!(m.get("mimeType"), Some("text/plain"));
        assert_eq!(m.get("trashed"), Some("true"));
        assert_eq!(m.parents, ["p1", "p2"]);
    }

    #[test]
    fn truncated_prefix_keeps_only_complete_values() {
        let m = parse_metadata(br#"{"name":"report.pdf","mimeType":"application/pd"#).unwrap();
        assert_eq!(m.get("name"), Some("report.pdf"));
        assert_eq!(m.get("mimeType"), None);
        let m = parse_metadata(br#"{"parents":["a","b"#).unwrap();
        assert_eq!(m.parents, ["a"]);
        let m = parse_metadata(br#"{"trashed":tr"#).unwrap();
        assert_eq!(m.get("trashed"), None);
    }

    #[test]
    fn non_json_is_rejected() {
        assert!(parse_metadata(b"hello world").is_none());
        assert!(parse_metadata(b"").is_none());
        assert!(parse_metadata(b"[1,2]").is_none());
    }

    #[test]
    fn multipart_first_part_is_the_json_metadata_only() {
        let body = b"--BND\r\ncontent-type: application/json; charset=UTF-8\r\n\r\n{\"name\":\"n.txt\",\"parents\":[\"f1\"]}\r\n--BND\r\ncontent-type: text/plain\r\n\r\nSECRET CONTENT\r\n--BND--";
        let part = multipart_first_part("multipart/related; boundary=BND", body).unwrap();
        assert_eq!(part, br#"{"name":"n.txt","parents":["f1"]}"#);
        let quoted = multipart_first_part("multipart/related; boundary=\"BND\"", body).unwrap();
        assert_eq!(quoted, part);
        // The first part is not JSON: refused rather than guessed.
        let txt = b"--BND\r\ncontent-type: text/plain\r\n\r\nSECRET\r\n--BND--";
        assert!(multipart_first_part("multipart/related; boundary=BND", txt).is_none());
        assert!(multipart_first_part("multipart/related", body).is_none());
    }

    #[test]
    fn multipart_cut_inside_the_metadata_part() {
        let body = b"--BND\r\ncontent-type: application/json\r\n\r\n{\"name\":\"n.txt\",\"mimeType\":\"app";
        let part = multipart_first_part("multipart/related; boundary=BND", body).unwrap();
        let m = parse_metadata(part).unwrap();
        assert_eq!(m.get("name"), Some("n.txt"));
        assert_eq!(m.get("mimeType"), None);
    }

    #[test]
    fn upload_summary_has_name_mime_parents_and_size_but_no_content() {
        let body = b"--BND\r\ncontent-type: application/json\r\n\r\n{\"name\":\"n.txt\",\"mimeType\":\"text/plain\",\"parents\":[\"f1\",\"f2\"]}\r\n--BND\r\ncontent-type: text/plain\r\n\r\nSECRET CONTENT\r\n--BND--";
        let q = kv([("uploadType", "multipart")]);
        let h = kv([
            ("content-type", "multipart/related; boundary=BND"),
            ("content-length", "4096"),
        ]);
        let b = build(&req("upload_file", "/upload/drive/v3/files", &q, &h, body, false)).unwrap();
        assert_eq!(b.0, "Upload Drive file");
        assert_eq!(field(&b, "Name"), Some("n.txt"));
        assert_eq!(field(&b, "Target MIME type"), Some("text/plain"));
        assert_eq!(field(&b, "Parent folder ids"), Some("f1, f2"));
        assert_eq!(field(&b, "Request size"), Some("4096 bytes"));
        assert!(!format!("{b:?}").contains("SECRET"));
    }

    #[test]
    fn media_upload_never_reads_the_body() {
        let q = kv([("uploadType", "media")]);
        let h = kv([("content-type", "application/pdf"), ("content-length", "12")]);
        let b = build(&req(
            "upload_file_content",
            "/upload/drive/v3/files/F9",
            &q,
            &h,
            b"{\"name\":\"CONTENT LOOKS LIKE JSON\"}",
            false,
        ))
        .unwrap();
        assert_eq!(field(&b, "File id"), Some("F9"));
        assert_eq!(field(&b, "Content type"), Some("application/pdf"));
        assert_eq!(field(&b, "Size"), Some("12 bytes"));
        assert_eq!(field(&b, "Name"), None);
    }

    #[test]
    fn resumable_start_uses_the_announced_size() {
        let q = kv([("uploadType", "resumable")]);
        let h = kv([
            ("x-upload-content-length", "1048576"),
            ("x-upload-content-type", "video/mp4"),
        ]);
        let b = build(&req(
            "upload_file",
            "/upload/drive/v3/files",
            &q,
            &h,
            br#"{"name":"clip.mp4"}"#,
            false,
        ))
        .unwrap();
        assert_eq!(field(&b, "Name"), Some("clip.mp4"));
        assert_eq!(field(&b, "Size"), Some("1048576 bytes"));
        assert_eq!(field(&b, "Content type"), Some("video/mp4"));
    }

    #[test]
    fn metadata_changes_and_moves() {
        let q = kv([("addParents", "dest1"), ("removeParents", "src1")]);
        let b = build(&req("update_file", "/drive/v3/files/F1", &q, &[], b"", false)).unwrap();
        assert_eq!(field(&b, "File id"), Some("F1"));
        assert_eq!(field(&b, "Add to folder ids"), Some("dest1"));
        assert_eq!(field(&b, "Remove from folder ids"), Some("src1"));
        let b = build(&req("update_file", "/drive/v3/files/F1", &[], &[], br#"{"trashed":true}"#, false))
            .unwrap();
        assert_eq!(field(&b, "Trashed"), Some("yes"));
        // Nothing to show: refused (the manifest template takes over).
        assert!(build(&req("update_file", "/drive/v3/files/F1", &[], &[], b"", false)).is_err());
    }

    #[test]
    fn downloads_and_exports() {
        let q = kv([("alt", "media")]);
        let b = build(&req("get_file", "/drive/v3/files/F2", &q, &[], b"", false)).unwrap();
        assert_eq!(b.0, "Download Drive file");
        assert_eq!(field(&b, "File id"), Some("F2"));
        // Metadata read: unsupported, not summarized.
        assert!(matches!(
            build(&req("get_file", "/drive/v3/files/F2", &[], &[], b"", false)),
            Err(HookError::Unsupported)
        ));
        let q = kv([("mimeType", "application/pdf")]);
        let b = build(&req("export_file", "/drive/v3/files/F3/export", &q, &[], b"", false)).unwrap();
        assert_eq!(field(&b, "File id"), Some("F3"));
        assert_eq!(field(&b, "Export MIME type"), Some("application/pdf"));
        assert!(build(&req("export_file", "/drive/v3/files/F3/export", &[], &[], b"", false)).is_err());
    }

    #[test]
    fn permissions() {
        let q = kv([("sendNotificationEmail", "false")]);
        let b = build(&req(
            "create_permission",
            "/drive/v3/files/F4/permissions",
            &q,
            &[],
            br#"{"role":"writer","type":"user","emailAddress":"bob@example.com","emailMessage":"private note"}"#,
            false,
        ))
        .unwrap();
        assert_eq!(field(&b, "Role"), Some("writer"));
        assert_eq!(field(&b, "Type"), Some("user"));
        assert_eq!(field(&b, "Email"), Some("bob@example.com"));
        assert_eq!(field(&b, "Notify by email"), Some("false"));
        assert!(!format!("{b:?}").contains("private note"));
        let b = build(&req(
            "create_permission",
            "/drive/v3/files/F4/permissions",
            &[],
            &[],
            br#"{"role":"reader","type":"domain","domain":"example.com"}"#,
            true,
        ))
        .unwrap();
        assert_eq!(field(&b, "Domain"), Some("example.com"));
        assert!(b.2.is_some());
        assert!(build(&req("create_permission", "/drive/v3/files/F4/permissions", &[], &[], b"x", false)).is_err());
    }

    #[test]
    fn values_are_one_line_and_clamped() {
        assert_eq!(clean("a\nb\u{7}c"), "a b c");
        assert!(clean(&"x".repeat(500)).chars().count() <= VALUE_MAX);
        let ids: Vec<String> = (0..8).map(|n| format!("id{n}")).collect();
        assert!(join_ids(&ids).ends_with("(+3 more)"));
    }

    #[test]
    fn unknown_tool_is_unsupported() {
        assert!(matches!(
            build(&req("list_files", "/drive/v3/files", &[], &[], b"", false)),
            Err(HookError::Unsupported)
        ));
    }
}
