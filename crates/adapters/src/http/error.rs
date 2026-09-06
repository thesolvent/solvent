//! Maps the domain error onto the response envelope. Client-input failures become `400` with the
//! (caller-safe) message; every other variant is an infrastructure failure that collapses to `500`
//! whose real cause is logged and whose body is generic — internals never reach the client.

use axum::http::StatusCode;
use solvent_core::SolventError;

use crate::http::primitives::Response;

/// Body returned for any `500` — the real error is logged, not sent.
const INTERNAL: &str = "Internal Error";

/// The HTTP status for a domain error. Only client-input is distinguished today; the rest are
/// infrastructure failures and collapse to `500` until a caller needs finer mapping (YAGNI).
fn status_for(err: &SolventError) -> StatusCode {
    match err {
        // Client input: a bad id, or a malformed/unverifiable order.
        SolventError::InvalidId { .. } | SolventError::Normalize(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl From<SolventError> for Response<()> {
    fn from(err: SolventError) -> Self {
        let status = status_for(&err);
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %err, "request failed");
            Response::error(INTERNAL, status)
        } else {
            Response::error(err.to_string(), status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::primitives::Status;
    use solvent_core::deps::registry::StoreError;

    #[test]
    fn invalid_id_is_a_400_with_the_message() {
        let err = SolventError::InvalidId {
            id_type: "trade",
            reason: "bad hex".to_string(),
        };
        let resp: Response<()> = err.into();
        assert_eq!(resp.status_code, StatusCode::BAD_REQUEST);
        assert_eq!(resp.status, Status::Error);
        assert_eq!(resp.error.as_deref(), Some("invalid trade id: bad hex"));
    }

    #[test]
    fn infra_errors_are_500_and_never_leak_internals() {
        let err = SolventError::from(StoreError::Db("secret-connection-string".to_string()));
        let resp: Response<()> = err.into();
        assert_eq!(resp.status_code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(resp.error.as_deref(), Some(INTERNAL));
        assert!(!resp.error.unwrap().contains("secret-connection-string"));
    }
}
