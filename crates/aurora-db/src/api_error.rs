//! `DbError` -> `ApiError` mapping (docs/02 §Error envelope).

use crate::DbError;
use aurora_core::ApiError;

impl From<DbError> for ApiError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound => ApiError::not_found("Not found."),
            DbError::UniqueViolation { .. } => ApiError::conflict("conflict", "Already exists."),
            other => {
                tracing::error!(error = %other, "database error");
                ApiError::internal("Internal error.")
            }
        }
    }
}

/// Map a unique violation to a domain code when its constraint is listed in
/// `codes` (`(constraint, code)`); everything else uses the default mapping.
pub fn conflict_code(e: DbError, codes: &[(&str, &str)]) -> ApiError {
    if let Some(c) = e.constraint()
        && matches!(e, DbError::UniqueViolation { .. })
        && let Some((_, code)) = codes.iter().find(|(k, _)| *k == c)
    {
        return ApiError::conflict(*code, "Already exists.");
    }
    e.into()
}
