//! REST response envelope — the shape every handler returns.
//!
//! Adopted from garden-rs `crates/api`: a `Response<T>` carrying `status` plus either `result`
//! (success) or `error` (failure). `status_code` is not serialized; it drives the HTTP status in
//! `IntoResponse`. Handlers return [`ApiResult`], so success and failure share one wire shape.

use alloy::primitives::Address;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::Json;
use serde::{Deserialize, Serialize};
use solvent_core::SolventError;

/// Parse a hex address, mapping a bad value to a `400` whose message names `field` as what was
/// invalid (`"token"`, `"wallet"`, …). The one address parser the handlers share.
pub fn parse_addr(field: &'static str, s: &str) -> Result<Address, SolventError> {
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: field,
        reason: e.to_string(),
    })
}

/// Whether an API call succeeded.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub enum Status {
    Ok,
    Error,
}

/// The envelope wrapping every response. `status_code` sets the HTTP status (never serialized).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct Response<T> {
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    #[schema(ignore)]
    pub status_code: StatusCode,
}

impl<T> Response<T> {
    /// A `200 OK` success carrying `data`.
    pub fn ok(data: T) -> Self {
        Self {
            status: Status::Ok,
            result: Some(data),
            error: None,
            status_code: StatusCode::OK,
        }
    }

    /// A failure carrying `error` and the given status code.
    pub fn error<E: ToString>(error: E, status_code: StatusCode) -> Self {
        Self {
            status: Status::Error,
            result: None,
            error: Some(error.to_string()),
            status_code,
        }
    }
}

impl<T: Serialize> IntoResponse for Response<T> {
    fn into_response(self) -> AxumResponse {
        let status_code = self.status_code;
        let mut response = Json(self).into_response();
        *response.status_mut() = status_code;
        response
    }
}

/// Handler return type: an `Ok` body or an error body, both `Response`-shaped.
pub type ApiResult<T> = Result<Response<T>, Response<()>>;

#[cfg(test)]
mod tests {
    use super::{Response, Status};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn ok_carries_result_and_no_error() {
        let body = Response::ok("data");
        assert_eq!(body.status, Status::Ok);
        assert_eq!(body.result, Some("data"));
        assert_eq!(body.error, None);
    }

    #[test]
    fn error_carries_message_and_no_result() {
        let body = Response::<String>::error("boom", StatusCode::BAD_REQUEST);
        assert_eq!(body.status, Status::Error);
        assert_eq!(body.result, None);
        assert_eq!(body.error.as_deref(), Some("boom"));
    }

    #[test]
    fn into_response_sets_status_and_json_content_type() {
        let response = Response::ok("data").into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
    }

    #[test]
    fn status_serializes_as_ok_or_error() {
        assert_eq!(serde_json::to_string(&Status::Ok).unwrap(), "\"Ok\"");
        assert_eq!(serde_json::to_string(&Status::Error).unwrap(), "\"Error\"");
    }
}
