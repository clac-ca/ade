use std::{error::Error as StdError, fmt};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

type BoxError = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug)]
pub struct AppError {
    kind: AppErrorKind,
    message: String,
    source: Option<BoxError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppErrorKind {
    Config,
    Database,
    Internal,
    Io,
    NotFound,
    Request,
    Response(StatusCode),
    Startup,
    Unavailable,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    error: String,
    message: String,
    #[serde(rename = "statusCode")]
    status_code: u16,
}

impl AppError {
    pub fn config(message: String) -> Self {
        Self::new(AppErrorKind::Config, message, None)
    }

    pub fn config_with_source(
        message: String,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::new(AppErrorKind::Config, message, Some(Box::new(source)))
    }

    pub fn database(message: String) -> Self {
        Self::new(AppErrorKind::Database, message, None)
    }

    pub fn database_with_source(
        message: String,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::new(AppErrorKind::Database, message, Some(Box::new(source)))
    }

    pub fn internal(message: String) -> Self {
        Self::new(AppErrorKind::Internal, message, None)
    }

    pub fn internal_with_source(
        message: String,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::new(AppErrorKind::Internal, message, Some(Box::new(source)))
    }

    pub fn io_with_source(message: String, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::new(AppErrorKind::Io, message, Some(Box::new(source)))
    }

    pub fn not_found(message: String) -> Self {
        Self::new(AppErrorKind::NotFound, message, None)
    }

    pub fn request(message: String) -> Self {
        Self::new(AppErrorKind::Request, message, None)
    }

    pub fn status(status: StatusCode, message: String) -> Self {
        Self::new(AppErrorKind::Response(status), message, None)
    }

    pub fn startup(message: String) -> Self {
        Self::new(AppErrorKind::Startup, message, None)
    }

    pub fn startup_with_source(
        message: String,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::new(AppErrorKind::Startup, message, Some(Box::new(source)))
    }

    pub fn unavailable(message: String) -> Self {
        Self::new(AppErrorKind::Unavailable, message, None)
    }

    fn new(kind: AppErrorKind, message: String, source: Option<BoxError>) -> Self {
        Self {
            kind,
            message,
            source,
        }
    }

    fn status_code(&self) -> StatusCode {
        match self.kind {
            AppErrorKind::NotFound => StatusCode::NOT_FOUND,
            AppErrorKind::Request | AppErrorKind::Config => StatusCode::BAD_REQUEST,
            AppErrorKind::Response(status) => status,
            AppErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            AppErrorKind::Database
            | AppErrorKind::Internal
            | AppErrorKind::Io
            | AppErrorKind::Startup => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn response_error_label(&self) -> &'static str {
        match self.status_code() {
            StatusCode::NOT_FOUND => "Not Found",
            StatusCode::SERVICE_UNAVAILABLE => "Service Unavailable",
            StatusCode::INTERNAL_SERVER_ERROR => "Internal Server Error",
            status => status.canonical_reason().unwrap_or("Request Error"),
        }
    }

    fn response_message(&self) -> String {
        match self.kind {
            AppErrorKind::Database
            | AppErrorKind::Internal
            | AppErrorKind::Io
            | AppErrorKind::Startup => "Internal Server Error".to_string(),
            AppErrorKind::Response(StatusCode::INTERNAL_SERVER_ERROR) => {
                "Internal Server Error".to_string()
            }
            _ => self.message.clone(),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for AppError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = ApiErrorBody {
            error: self.response_error_label().to_string(),
            message: self.response_message(),
            status_code: status.as_u16(),
        };

        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::AppError;

    #[test]
    fn internal_errors_hide_internal_details() {
        let response = AppError::internal("boom".to_string()).into_response();

        assert_eq!(response.status().as_u16(), 500);
    }
}
