//! The list/read items of C-1143 (P6-TPL-6): a helper file is named, sized and hashed; the
//! bytes themselves only ever travel through the content route.

use aurora_db::repo::agent_helpers as repo;
use serde::Serialize;
use utoipa::ToSchema;

/// One helper file of an agent, as the list route reports it (C-1143).
#[derive(Debug, Serialize, ToSchema)]
pub struct HelperFile {
    /// The C-1141 helper path below `helpers/` (projected at `.aurora/helpers/<path>`).
    pub path: String,
    /// Byte count.
    pub size: i32,
    pub executable: bool,
    /// Hex SHA-256 digest of the content.
    pub sha256: String,
}

/// The list route's body: `{ items: [...] }` (C-1143).
#[derive(Debug, Serialize, ToSchema)]
pub struct HelperList {
    pub items: Vec<HelperFile>,
}

impl From<repo::HelperRow> for HelperFile {
    fn from(r: repo::HelperRow) -> Self {
        Self {
            path: r.path,
            size: r.size,
            executable: r.executable,
            sha256: r.sha256,
        }
    }
}
