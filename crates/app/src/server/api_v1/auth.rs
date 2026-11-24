//! Authentication of `/api/v1`: `Authorization: Bearer tr_live_...` only. Cookies and sessions
//! are never read here, so a browser session cannot reach the API (and the cross-site check of
//! the browser endpoints does not apply). Every authenticated call is rate limited per key and
//! audited as `api.call`: by this layer for reads and for refused calls, by the handler itself,
//! inside the transaction of the change, for a successful mutation. A rejected key is audited as
//! `api.auth_failed` with a keyed hash of the client, at a bounded rate.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, MatchedPath, Request, State};
use axum::http::Method;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use sqlx::PgExecutor;
use store::repo::api_key::{self, ApiKey, ApiScope};
use store::repo::audit;
use uuid::Uuid;

use super::error::ApiError;
use crate::server::AppState;
use crate::server::auth::http::RateLimiter;

/// Requests per minute and key.
pub const RATE_LIMIT_PER_MINUTE: u32 = 120;

/// Audit rows for rejected keys per client in [`FAILED_AUDIT_WINDOW`].
const FAILED_AUDIT_PER_CLIENT: u32 = 5;
/// Audit rows for rejected keys from all clients together in [`FAILED_AUDIT_WINDOW`].
const FAILED_AUDIT_TOTAL: u32 = 60;
const FAILED_AUDIT_WINDOW: Duration = Duration::from_secs(600);

/// What the API handlers share: the application state, the per-key rate limiter and the limiters
/// that keep failed authentications from flooding the audit log.
#[derive(Clone)]
pub struct ApiState {
    pub app: AppState,
    pub limiter: Arc<RateLimiter>,
    failed_per_client: Arc<RateLimiter>,
    failed_total: Arc<RateLimiter>,
}

impl ApiState {
    pub fn new(app: AppState) -> Self {
        Self::with_limit(app, RATE_LIMIT_PER_MINUTE)
    }

    pub fn with_limit(app: AppState, per_minute: u32) -> Self {
        Self {
            app,
            limiter: Arc::new(RateLimiter::new(per_minute)),
            failed_per_client: Arc::new(RateLimiter::with_window(
                FAILED_AUDIT_PER_CLIENT,
                FAILED_AUDIT_WINDOW,
            )),
            failed_total: Arc::new(RateLimiter::with_window(
                FAILED_AUDIT_TOTAL,
                FAILED_AUDIT_WINDOW,
            )),
        }
    }

    /// Keyed hash of the client address (never the address itself), or `unknown`.
    fn client_hash(&self, req: &Request) -> String {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let ip = if self.app.config.trust_proxy_headers {
            req.headers()
                .get_all("x-forwarded-for")
                .iter()
                .next_back()
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
                .or(peer)
        } else {
            peer
        };
        let Some(ip) = ip else {
            return "unknown".to_string();
        };
        let network = match ip.to_canonical() {
            IpAddr::V6(ip) => {
                let s = ip.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
            ip => ip.to_string(),
        };
        self.app
            .blind
            .index(&format!("api-client\u{1f}{network}"))
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Audits a rejected key, unless this client or the server as a whole already used up its
    /// share of rows. Best effort: the answer is a 401 either way.
    async fn audit_failed_key(&self, client: String) {
        let now = Instant::now();
        if !(self.failed_per_client.allow(&client, now) && self.failed_total.allow("all", now)) {
            return;
        }
        if let Err(e) = audit::record(
            &self.app.pool,
            None,
            "api.auth_failed",
            "api_client",
            None,
            &[&format!("client:{client}")],
        )
        .await
        {
            tracing::error!(error = %e, "api v1: cannot audit a rejected key");
        }
    }
}

/// Appends the `api.call` row of a call: the route pattern and the status, never values. A
/// handler that mutates data passes its transaction so the row commits (or not) with the change.
pub async fn record_call<'e>(
    executor: impl PgExecutor<'e>,
    key_id: Uuid,
    route: &str,
    status: u16,
) -> Result<(), store::repo::RepoError> {
    audit::record(
        executor,
        None,
        "api.call",
        "api_key",
        Some(key_id),
        &[&format!("path:{route}"), &format!("status:{status}")],
    )
    .await
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
                let client = state.client_hash(&req);
                state.audit_failed_key(client).await;
                return ApiError::Unauthorized.into_response();
            }
            Err(e) => return ApiError::from(e).into_response(),
        },
        None => return ApiError::Unauthorized.into_response(),
    };
    let mutating = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
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
    // A successful mutation was audited by its handler, in the transaction of the change.
    if mutating && status.is_success() {
        return res;
    }
    // Fails closed: when the call cannot be audited its answer is not delivered.
    if let Err(e) = record_call(&state.app.pool, key.id, &route, status.as_u16()).await {
        tracing::error!(error = %e, "api v1: cannot audit a call");
        return ApiError::Internal.into_response();
    }
    res
}
