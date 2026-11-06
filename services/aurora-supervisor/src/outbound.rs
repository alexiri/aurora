//! Outbound files (`docs/12-supervisor-harness.md` §Outbound files): validate the path, split into
//! `file.part` chunks (<= 144 KiB decoded) and finish with `file.commit` (sha256 + size).

use std::path::{Path, PathBuf};

use aurora_core::limits::ATTACH_OUT_FILE_MAX;
use aurora_protocol::supervisor::{
    FileCommit, FilePart, MAX_FILE_PART_DECODED, SupervisorToRunner,
};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::channel::ChannelSender;

#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    #[error("path is outside the home or not a regular file")]
    BadPath,
    /// Inside the platform-managed `.aurora/` directory (tool cache, inbox, projections).
    #[error("path is inside the platform directory .aurora/")]
    Reserved,
    #[error("file larger than {0} bytes")]
    TooLarge(u64),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("channel closed")]
    Closed,
}

#[derive(Debug, Clone)]
pub struct OutboundFile {
    /// Absolute, or relative to the home.
    pub path: PathBuf,
    pub conversation_id: Uuid,
    pub turn_id: Uuid,
    pub caption: Option<String>,
}

/// Platform-managed directory under the home that `send_file` never reads (12 §Outbound files).
pub const RESERVED_DIR: &str = ".aurora";

/// Uploads one file; returns the `upload_id` the runner answers `file.result` with.
pub async fn send_file(
    out: &ChannelSender,
    home: &Path,
    req: OutboundFile,
) -> Result<String, OutboundError> {
    let root = tokio::fs::canonicalize(home).await?;
    let real = tokio::fs::canonicalize(root.join(&req.path))
        .await
        .map_err(|_| OutboundError::BadPath)?;
    if !real.starts_with(&root) {
        return Err(OutboundError::BadPath);
    }
    // Checked on the canonical path, so `..` segments and symlinks into it are refused too.
    if real.starts_with(root.join(RESERVED_DIR)) {
        return Err(OutboundError::Reserved);
    }
    let meta = tokio::fs::metadata(&real).await?;
    if !meta.is_file() {
        return Err(OutboundError::BadPath);
    }
    if meta.len() > ATTACH_OUT_FILE_MAX {
        return Err(OutboundError::TooLarge(ATTACH_OUT_FILE_MAX));
    }
    let filename: String = real
        .file_name()
        .map(|n| n.to_string_lossy().chars().take(255).collect())
        .ok_or(OutboundError::BadPath)?;
    let upload_id = Uuid::now_v7().to_string();
    let mut file = tokio::fs::File::open(&real).await?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut index = 0u32;
    let mut buf = vec![0u8; MAX_FILE_PART_DECODED];
    loop {
        let mut filled = 0;
        while filled < buf.len() {
            let n = file.read(&mut buf[filled..]).await?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        size += filled as u64;
        if size > ATTACH_OUT_FILE_MAX {
            return Err(OutboundError::TooLarge(ATTACH_OUT_FILE_MAX));
        }
        hasher.update(&buf[..filled]);
        let part = FilePart {
            upload_id: upload_id.clone(),
            index,
            data_b64: base64::engine::general_purpose::STANDARD.encode(&buf[..filled]),
        };
        out.send(SupervisorToRunner::FilePart(part))
            .await
            .map_err(|_| OutboundError::Closed)?;
        index += 1;
        if filled < buf.len() {
            break;
        }
    }
    out.send(SupervisorToRunner::FileCommit(FileCommit {
        upload_id: upload_id.clone(),
        conversation_id: req.conversation_id,
        turn_id: req.turn_id,
        filename,
        caption: req.caption,
        sha256: hex::encode(hasher.finalize()),
        size,
    }))
    .await
    .map_err(|_| OutboundError::Closed)?;
    Ok(upload_id)
}
