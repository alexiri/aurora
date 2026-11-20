//! Wire shapes of the Files routes (C-898).
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Where a symlink points (`target` appears only on symlink entries).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FileTarget {
    pub path: String,
    /// `file` | `dir` | `other` | `missing`.
    pub kind: String,
    pub outside_workspace: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    /// `file` | `dir` | `symlink` | `other`.
    pub kind: String,
    pub size: u64,
    pub modified_at: Option<DateTime<Utc>>,
    /// Octal permission bits only (`"0644"`).
    pub mode: String,
    /// Owner write bit set and not read-only per C-899.
    pub writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<FileTarget>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FileList {
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FileText {
    pub path: String,
    pub content: String,
    /// `"sha256:" + hex(SHA-256 of the bytes)`. Null only for a preview of a file over the editor
    /// limit (`truncated: true`, P5-REFINE-4, C-1060): such a preview can never be saved.
    pub etag: Option<String>,
    /// The whole file's size (also when `content` is a preview of its start).
    pub size: u64,
    pub modified_at: Option<DateTime<Utc>>,
    /// `lf` | `crlf`.
    pub line_ending: String,
    /// (P5-REFINE-4, C-1060) `content` is the file's first 256 KiB cut at a line end
    /// (`preview=1` on a file over 1 MiB); false for the whole file.
    pub truncated: bool,
}

/// `GET …/files/text` (C-898; `preview`, C-1060).
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct TextQuery {
    /// Absolute path under `/workspace`.
    pub path: Option<String>,
    /// (P5-REFINE-4, C-1060) `1`/`true`: a text file over the editor limit (1 MiB) answers its
    /// first 256 KiB (`truncated: true`, no `etag`) instead of `422 file_not_editable`.
    #[param(inline)]
    pub preview: Option<PreviewFlag>,
}

/// `preview=1` / `preview=true` (and `0` / `false`).
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
pub enum PreviewFlag {
    #[serde(rename = "1", alias = "true")]
    On,
    #[serde(rename = "0", alias = "false")]
    Off,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FileWriteResult {
    pub entry: FileEntry,
    /// Present when the size is at most `FILES_EDIT_MAX`.
    pub etag: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct PathQuery {
    /// Absolute path under `/workspace` (absent on a listing = `/workspace`).
    pub path: Option<String>,
}

/// `POST …/files/ops`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    Mkdir {
        path: String,
        #[serde(default)]
        parents: bool,
    },
    Rename {
        path: String,
        to: String,
    },
    Delete {
        path: String,
        #[serde(default)]
        recursive: bool,
    },
}

/// Result of an op: `entry` for mkdir and rename, `deleted` for delete.
#[derive(Debug, Serialize, ToSchema)]
pub struct FileOpResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<FileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<u64>,
}
