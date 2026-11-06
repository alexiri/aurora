//! Registry compile hook. BE-041 implements the compile + `aurora_registry` NOTIFY;
//! BE-040 only calls it after the set of active packages changed.

use crate::app::AppState;
use aurora_core::ApiError;

/// Ask the registry to recompile the active snapshot (stub until BE-041).
pub async fn request_compile(_state: &AppState) -> Result<(), ApiError> {
    Ok(())
}
