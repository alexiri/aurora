use sqlx::error::ErrorKind;

/// Database error with constraint violations classified.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("unique violation on {constraint:?}")]
    UniqueViolation { constraint: Option<String> },
    #[error("foreign key violation on {constraint:?}")]
    ForeignKeyViolation { constraint: Option<String> },
    #[error("check violation on {constraint:?}")]
    CheckViolation { constraint: Option<String> },
    #[error("not found")]
    NotFound,
    #[error("migration failed: {0}")]
    Migrate(#[source] sqlx::migrate::MigrateError),
    #[error("database error: {0}")]
    Other(#[source] sqlx::Error),
}

impl DbError {
    /// Constraint name for violation variants.
    pub fn constraint(&self) -> Option<&str> {
        match self {
            Self::UniqueViolation { constraint }
            | Self::ForeignKeyViolation { constraint }
            | Self::CheckViolation { constraint } => constraint.as_deref(),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        if matches!(e, sqlx::Error::RowNotFound) {
            return Self::NotFound;
        }
        if let Some(db) = e.as_database_error() {
            let constraint = db.constraint().map(str::to_owned);
            match db.kind() {
                ErrorKind::UniqueViolation => return Self::UniqueViolation { constraint },
                ErrorKind::ForeignKeyViolation => return Self::ForeignKeyViolation { constraint },
                ErrorKind::CheckViolation => return Self::CheckViolation { constraint },
                _ => {}
            }
        }
        Self::Other(e)
    }
}
