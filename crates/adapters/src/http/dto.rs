//! Shared wire types for list endpoints: the `List<T>` payload, the `Page` query extractor, and an
//! opaque cursor codec. Payload value types (`Amount`, `Token`) join this module at their first
//! consumer in M1.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use solvent_core::SolventError;

/// A page of a collection: the items plus an opaque `next_cursor` (absent on the last page) and an
/// optional `total`. Carried inside the response envelope's `result`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct List<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl<T> List<T> {
    /// A page carrying a continuation cursor and/or a total.
    pub fn page(items: Vec<T>, next_cursor: Option<String>, total: Option<u64>) -> Self {
        Self {
            items,
            next_cursor,
            total,
        }
    }

    /// The whole collection in one page — no cursor.
    pub fn all(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
            total: None,
        }
    }
}

/// Pagination query params (`?limit=&cursor=`), extracted with `Query<Page>`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Page {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

impl Page {
    const DEFAULT_LIMIT: u32 = 50;
    const MAX_LIMIT: u32 = 200;

    /// The page size to use: the client's `limit`, clamped to `[1, MAX_LIMIT]`, else the default.
    pub fn limit(&self) -> u32 {
        self.limit
            .unwrap_or(Self::DEFAULT_LIMIT)
            .clamp(1, Self::MAX_LIMIT)
    }

    /// The decoded cursor key, if the client sent one. A malformed cursor surfaces as a `400`.
    pub fn cursor<T: DeserializeOwned>(&self) -> Result<Option<T>, SolventError> {
        self.cursor.as_deref().map(Cursor::decode).transpose()
    }
}

/// Opaque, URL-safe cursor: `base64url(json(key))`. Each endpoint chooses its own key type; the
/// client only ever sees the encoded string.
pub struct Cursor;

impl Cursor {
    /// Encode a key into an opaque cursor. Serialization is infallible for cursor keys (no floats,
    /// no non-string map keys), so a failure here is a programmer error, not a runtime condition.
    pub fn encode<T: Serialize>(key: &T) -> String {
        let json = serde_json::to_vec(key).expect("cursor key serializes to JSON");
        URL_SAFE_NO_PAD.encode(json)
    }

    /// Decode a cursor back into its key. A malformed cursor is client input → `InvalidId` (400).
    pub fn decode<T: DeserializeOwned>(s: &str) -> Result<T, SolventError> {
        let bytes = URL_SAFE_NO_PAD.decode(s).map_err(|e| invalid_cursor(&e))?;
        serde_json::from_slice(&bytes).map_err(|e| invalid_cursor(&e))
    }
}

fn invalid_cursor(err: &dyn std::fmt::Display) -> SolventError {
    SolventError::InvalidId {
        id_type: "cursor",
        reason: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        let key = ("0xabc".to_string(), 42u64);
        let encoded = Cursor::encode(&key);
        let decoded: (String, u64) = Cursor::decode(&encoded).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn malformed_cursor_is_invalid_id() {
        let err = Cursor::decode::<String>("!!! not base64 !!!").unwrap_err();
        assert!(matches!(
            err,
            SolventError::InvalidId {
                id_type: "cursor",
                ..
            }
        ));
    }

    #[test]
    fn limit_clamps_to_bounds() {
        let page = |limit| Page {
            limit,
            cursor: None,
        };
        assert_eq!(page(None).limit(), 50);
        assert_eq!(page(Some(0)).limit(), 1);
        assert_eq!(page(Some(10)).limit(), 10);
        assert_eq!(page(Some(9999)).limit(), 200);
    }
}
