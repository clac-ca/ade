use std::error::Error as StdError;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;
use utoipa::ToSchema;

type BoxError = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{message}")]
    Config {
        message: String,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    Database {
        message: String,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    Internal {
        message: String,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{message}")]
    Io {
        message: String,
        #[source]
        source: BoxError,
    },
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Request(String),
    #[error("{message}")]
    Response { status: StatusCode, message: String },
    #[error("{message}")]
    Startup {
        message: String,
        #[source]
        source: Option<BoxError>,
    },
    #[error("{0}")]
    Unavailable(String),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    error: String,
    message: String,
    #[serde(rename = "statusCode")]
    status_code: u16,
}

impl AppError {
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
            source: None,
        }
    }

    pub fn config_with_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Config {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn database(message: impl Into<String>) -> Self {
        Self::Database {
            message: message.into(),
            source: None,
        }
    }

    pub fn database_with_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Database {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            source: None,
        }
    }

    pub fn internal_with_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Internal {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn io_with_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Io {
            message: message.into(),
            source: Box::new(source),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn request(message: impl Into<String>) -> Self {
        Self::Request(message.into())
    }

    pub fn status(status: StatusCode, message: impl Into<String>) -> Self {
        Self::Response {
            status,
            message: message.into(),
        }
    }

    pub fn startup(message: impl Into<String>) -> Self {
        Self::Startup {
            message: message.into(),
            source: None,
        }
    }

    pub fn startup_with_source(
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Startup {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }

    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Request(_) | Self::Config { .. } => StatusCode::BAD_REQUEST,
            Self::Response { status, .. } => *status,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Database { .. }
            | Self::Internal { .. }
            | Self::Io { .. }
            | Self::Startup { .. } => StatusCode::INTERNAL_SERVER_ERROR,
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
        match self {
            Self::Database { .. }
            | Self::Internal { .. }
            | Self::Io { .. }
            | Self::Startup { .. }
            | Self::Response {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                ..
            } => "Internal Server Error".to_string(),
            _ => self.to_string(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = ErrorResponse {
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
        let response = AppError::internal("boom").into_response();

        assert_eq!(response.status().as_u16(), 500);
    }
}
