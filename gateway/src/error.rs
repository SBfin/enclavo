use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::x402::types::PaymentRequired;

/// Errors returned to clients, shaped like OpenAI errors so SDKs surface them cleanly.
#[derive(Debug)]
pub enum ApiError {
    Status {
        status: StatusCode,
        kind: &'static str,
        code: &'static str,
        message: String,
    },
    /// The upstream rejected the request itself (4xx); its body is passed through.
    Upstream {
        status: StatusCode,
        body: Value,
    },
    PaymentRequired(Box<PaymentRequired>),
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::BAD_REQUEST,
            kind: "invalid_request_error",
            code: "invalid_request",
            message: message.into(),
        }
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::UNAUTHORIZED,
            kind: "authentication_error",
            code: "invalid_api_key",
            message: message.into(),
        }
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::NOT_FOUND,
            kind: "invalid_request_error",
            code: "not_found",
            message: message.into(),
        }
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::CONFLICT,
            kind: "invalid_request_error",
            code: "conflict",
            message: message.into(),
        }
    }
    pub fn rate_limited() -> Self {
        Self::Status {
            status: StatusCode::TOO_MANY_REQUESTS,
            kind: "rate_limit_error",
            code: "rate_limited",
            message: "Too many requests; slow down and retry.".into(),
        }
    }
    pub fn no_route(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: "server_error",
            code: "no_route_available",
            message: message.into(),
        }
    }
    pub fn upstream_failed(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::BAD_GATEWAY,
            kind: "server_error",
            code: "upstream_failed",
            message: message.into(),
        }
    }
    pub fn payment_failed(message: impl Into<String>) -> Self {
        Self::Status {
            status: StatusCode::PAYMENT_REQUIRED,
            kind: "payment_error",
            code: "payment_failed",
            message: message.into(),
        }
    }
    pub fn payment_unavailable() -> Self {
        Self::Status {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: "server_error",
            code: "payment_unavailable",
            message: "The payment facilitator is unavailable; retry shortly.".into(),
        }
    }
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::Status {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: "server_error",
            code: "internal_error",
            message: "Internal error.".into(),
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Status { status, .. } | Self::Upstream { status, .. } => *status,
            Self::PaymentRequired(_) => StatusCode::PAYMENT_REQUIRED,
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::internal(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Status { status, kind, code, message } => {
                (status, Json(json!({ "error": { "message": message, "type": kind, "code": code } }))).into_response()
            }
            Self::Upstream { status, body } => (status, Json(body)).into_response(),
            Self::PaymentRequired(required) => {
                let mut response = (StatusCode::PAYMENT_REQUIRED, Json(&*required)).into_response();
                if let Ok(value) = HeaderValue::from_str(&required.to_header()) {
                    response.headers_mut().insert(crate::x402::types::PAYMENT_REQUIRED_HEADER, value);
                }
                response
            }
        }
    }
}
