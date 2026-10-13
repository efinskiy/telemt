use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug)]
pub(crate) struct ApiError(pub StatusCode, pub &'static str);
pub(crate) type Result<T> = std::result::Result<T, ApiError>;
impl IntoResponse for ApiError {
    fn into_response(self) -> Response { (self.0, Json(json!({"ok":false,"error":{"code":self.1}}))).into_response() }
}
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(database) = error.as_database_error() {
            let code = database.code();
            return match code.as_deref() {
                Some("23505") => Self(StatusCode::CONFLICT, "resource_exists"),
                Some("23503") => Self(StatusCode::CONFLICT, "resource_in_use"),
                Some("23514") => Self(StatusCode::BAD_REQUEST, "validation_or_capacity"),
                _ => { tracing::error!(code = code.as_deref().unwrap_or("unknown"), "database operation failed"); Self(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable") }
            };
        }
        tracing::error!("database operation unavailable");
        Self(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable")
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(_: anyhow::Error) -> Self { Self(StatusCode::BAD_GATEWAY, "upstream_operation_failed") }
}
pub(crate) fn bad() -> ApiError { ApiError(StatusCode::BAD_REQUEST, "invalid_request") }
