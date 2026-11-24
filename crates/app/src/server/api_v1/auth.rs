//! Authentication of `/api/v1`: `Authorization: Bearer tr_live_...` only. Cookies and sessions
//! are never read here, so a browser session cannot reach the API (and the cross-site check of
//! the browser endpoints does not apply). Every authenticated call is rate limited per key and
//! audited as `api.call`.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{FromRequestParts, MatchedPath, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use store::repo::api_key::{self, ApiKey, ApiScope};
use store::repo::audit;

use super::error::ApiError;
use crate::server::AppState;
use crate::server::auth::http::RateLimiter;

/// Requests per minute and key.
pub const RATE_LIMIT_PER_MINUTE: u32 = 120;

/// What the API handlers share: the application state and the per-key rate limiter.
#[derive(Clone)]
pub struct ApiState {
    pub app: AppState,
    pub limiter: Arc<RateLimiter>,
}

impl ApiState {
    pub fn new(app: AppState) -> Self {
        Self::with_limit(app, RATE_LIMIT_PER_MINUTE)
    }

    pub fn with_limit(app: AppState, per_minute: u32) -> Self {
        Self {
            app,
            limiter: Arc::new(RateLimiter::new(per_minute)),
        }
    }
}

/// The key behind the current request, put there by [`authenticate`].
#[derive(Clone)]
pub struct ApiAuth(pub ApiKey);

impl ApiAuth {
    /// Refuses (403) a key without `scope`.
    pub fn require(&self, scope: ApiScope) -> Result<(), ApiError> {
        if self.0.has_scope(scope) {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ApiAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        parts
            .extensions
            .get::<ApiAuth>()
            .cloned()
            .ok_or(ApiError::Unauthorized)
    }
}

/// The token of an `Authorization: Bearer <token>` header.
fn bearer(req: &Request) -> Option<&str> {
    let value = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|t| !t.is_empty())
}

pub async fn authenticate(State(state): State<ApiState>, mut req: Request, next: Next) -> Response {
    let key = match bearer(&req) {
        Some(token) => match api_key::verify(&state.app.pool, token, Utc::now()).await {
            Ok(Some(key)) => key,
            Ok(None) => {
                tracing::warn!("api v1: unknown, revoked or expired key");
                return ApiError::Unauthorized.into_response();
            }
            Err(e) => return ApiError::from(e).into_response(),
        },
        None => return ApiError::Unauthorized.into_response(),
    };
    // The route pattern, not the URL: path parameters may carry identifiers of people.
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_string();
    let res = if state.limiter.allow(&key.id.to_string(), Instant::now()) {
        req.extensions_mut().insert(ApiAuth(key.clone()));
        next.run(req).await
    } else {
        ApiError::RateLimited.into_response()
    };
    let status = res.status();
    // Fails closed: when the call cannot be audited its answer is not delivered.
    if let Err(e) = audit::record(
        &state.app.pool,
        None,
        "api.call",
        "api_key",
        Some(key.id),
        &[
            &format!("path:{route}"),
            &format!("status:{}", status.as_u16()),
        ],
    )
    .await
    {
        tracing::error!(error = %e, "api v1: cannot audit a call");
        return ApiError::Internal.into_response();
    }
    res
}
