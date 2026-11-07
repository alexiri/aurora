//! Cursor pagination: `?limit=` (default 50, max 200), opaque `?cursor=`,
//! order `created_at desc, id desc`; fetch `limit + 1` rows and call [`Page::from_rows`].

use aurora_core::{ApiError, FieldError};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 200;

/// Position of the last item returned: `{"t": created_at, "id": uuid}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub t: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap_or_default())
    }
    pub fn decode(s: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(s).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Standard query parameters. Domains with extra filters declare their own
/// `limit`/`cursor` fields and call [`Pagination::new`].
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageParams {
    /// Page size, 1..=200 (default 50).
    pub limit: Option<u32>,
    /// Opaque cursor from a previous response's `next_cursor`.
    pub cursor: Option<String>,
}

/// Validated pagination input.
#[derive(Debug, Clone, Copy)]
pub struct Pagination {
    pub limit: u32,
    pub after: Option<Cursor>,
}

impl Pagination {
    pub fn new(limit: Option<u32>, cursor: Option<&str>) -> Result<Self, ApiError> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ApiError::invalid_request(
                "Invalid limit.",
                vec![FieldError::new("/limit", "must be between 1 and 200")],
            ));
        }
        let after = match cursor {
            None => None,
            Some(c) => Some(Cursor::decode(c).ok_or_else(|| {
                ApiError::invalid_request(
                    "Invalid cursor.",
                    vec![FieldError::new("/cursor", "malformed cursor")],
                )
            })?),
        };
        Ok(Self { limit, after })
    }

    /// Rows to request from the database (`limit + 1`).
    pub fn fetch(&self) -> i64 {
        i64::from(self.limit) + 1
    }
}

impl PageParams {
    pub fn resolve(&self) -> Result<Pagination, ApiError> {
        Pagination::new(self.limit, self.cursor.as_deref())
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// Build from up to `limit + 1` rows; `key` yields `(created_at, id)`.
    pub fn from_rows(
        mut rows: Vec<T>,
        limit: u32,
        key: impl Fn(&T) -> (DateTime<Utc>, Uuid),
    ) -> Self {
        let limit = limit as usize;
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|r| {
                let (t, id) = key(r);
                Cursor { t, id }.encode()
            })
        } else {
            None
        };
        Page {
            items: rows,
            next_cursor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_and_page() {
        let rows: Vec<(DateTime<Utc>, Uuid)> =
            (0..4).map(|_| (Utc::now(), Uuid::now_v7())).collect();
        let last = rows[1];
        let page = Page::from_rows(rows, 2, |r| *r);
        assert_eq!(page.items.len(), 2);
        let c = Cursor::decode(page.next_cursor.as_deref().unwrap()).unwrap();
        assert_eq!((c.t, c.id), last);
        let p = Pagination::new(Some(2), page.next_cursor.as_deref()).unwrap();
        assert_eq!(p.after, Some(c));
        assert_eq!(p.fetch(), 3);
        let short = Page::from_rows(vec![1], 2, |_| (Utc::now(), Uuid::nil()));
        assert!(short.next_cursor.is_none());
    }

    #[test]
    fn validation() {
        assert_eq!(Pagination::new(None, None).unwrap().limit, 50);
        assert!(Pagination::new(Some(0), None).is_err());
        assert!(Pagination::new(Some(201), None).is_err());
        assert!(Pagination::new(None, Some("!!")).is_err());
    }
}
