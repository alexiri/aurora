use aurora_db::repo::attachments::AttachmentRow;
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// One attachment (API_CONTRACTS C-30).
#[derive(Serialize, ToSchema)]
pub struct Attachment {
    pub id: Uuid,
    pub conversation_id: Uuid,
    /// Set once bound to a turn (outbound files: the turn that sent them).
    pub turn_id: Option<Uuid>,
    /// `in` (people -> agent) | `out` (agent -> people)
    pub direction: String,
    /// `pending` | `bound` | `failed`
    pub status: String,
    /// Sanitized to `[A-Za-z0-9._-]`, at most 128 characters.
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub caption: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<AttachmentRow> for Attachment {
    fn from(r: AttachmentRow) -> Self {
        Self {
            id: r.id,
            conversation_id: r.conversation_id,
            turn_id: r.turn_id,
            direction: r.direction,
            status: r.status,
            filename: r.filename,
            content_type: r.content_type,
            size_bytes: r.size_bytes,
            caption: r.caption,
            created_at: r.created_at,
        }
    }
}

/// `multipart/form-data` body of the upload route.
#[derive(ToSchema)]
pub struct UploadAttachmentRequest {
    /// The file, at most 10 MiB.
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
}
