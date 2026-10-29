//! A failed request: a message for the caller on a client error, a bare 500 on an internal one.

use super::*;

/// A failed request. Client errors carry a message for the caller; internal ones are
/// logged in full and reported as a bare 500.
pub(super) enum ApiError {
    Client(StatusCode, String),
    Internal(anyhow::Error),
}

impl ApiError {
    pub(super) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self::Client(status, message.into())
    }
}

/// A 400 with `message` for the caller.
pub(super) fn bad_request(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        Self::Internal(err.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Client(status, message) => {
                (status, Json(json!({ "error": message }))).into_response()
            }
            Self::Internal(err) => {
                tracing::error!(error = format!("{err:#}"), "request failed");
                let body = Json(json!({ "error": "internal error" }));
                (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
            }
        }
    }
}
