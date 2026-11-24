//! JSON errors of the public API: `{ "error": "code" }` with a stable, machine-readable code.

use axum::Json;
use axum::http::header::{RETRY_AFTER, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    Unauthorized,
    Forbidden,
    NotFound,
    /// The request is valid but the state of the data forbids it (an export of unvalidated
    /// days, a payroll number that belongs to someone with a role that the API cannot change).
    Conflict,
    RateLimited,
    InvalidRequest,
    Internal,
}

/// Error body of every non-2xx response.
#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    /// Stable code: `unauthorized`, `forbidden`, `not_found`, `conflict`, `rate_limited`,
    /// `invalid_request` or `internal`.
    #[schema(example = "forbidden")]
    pub error: String,
}

impl ApiError {
    pub fn status(self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::InvalidRequest => "invalid_request",
            Self::Internal => "internal",
        }
    }
}

impl From<crate::server::export::ExportError> for ApiError {
    fn from(e: crate::server::export::ExportError) -> Self {
        use crate::server::export::ExportError as E;
        match e {
            E::Forbidden => Self::Forbidden,
            E::NotFound => Self::NotFound,
            // Also a period with more rows than one export may hold.
            E::Invalid(_) | E::TooLarge => Self::InvalidRequest,
            E::Busy => Self::RateLimited,
            // Nothing is produced; GET /periods says which sites and days are not validated.
            E::Blocked(_) | E::ProfileInvalid(_) => Self::Conflict,
            E::Internal => Self::Internal,
        }
    }
}

impl From<crate::server::hours::HoursError> for ApiError {
    fn from(e: crate::server::hours::HoursError) -> Self {
        crate::server::export::ExportError::from(e).into()
    }
}

impl From<store::repo::RepoError> for ApiError {
    fn from(e: store::repo::RepoError) -> Self {
        // Repository errors carry no personal data, so they may be logged.
        tracing::error!(error = %e, "api v1 request failed");
        Self::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: self.code().to_string(),
        });
        let mut res = (self.status(), body).into_response();
        match self {
            Self::Unauthorized => {
                res.headers_mut()
                    .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            }
            Self::RateLimited => {
                res.headers_mut()
                    .insert(RETRY_AFTER, HeaderValue::from_static("60"));
            }
            _ => {}
        }
        res
    }
}
