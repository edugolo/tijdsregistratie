//! Guards for server functions. Errors are generic: they never reveal whether data exists.

use dioxus::prelude::ServerFnError;
use sqlx::PgPool;
use uuid::Uuid;

use super::session::{SessionKind, SessionUser};
use super::{app_state, session};
use store::repo::role::{self, StaffScope, scope_allows_sites, scope_from_roles};

fn http_error(code: u16, message: &str) -> ServerFnError {
    ServerFnError::ServerError {
        message: message.to_string(),
        code,
        details: None,
    }
}

fn unauthorized() -> ServerFnError {
    http_error(401, "unauthorized")
}

fn forbidden() -> ServerFnError {
    http_error(403, "forbidden")
}

fn internal(e: impl std::fmt::Display) -> ServerFnError {
    tracing::error!(error = %e, "server function failed");
    http_error(500, "internal error")
}

/// Whether `person_id` falls within the scope.
pub async fn scope_allows_person(
    scope: &StaffScope,
    person_id: Uuid,
) -> Result<bool, ServerFnError> {
    if scope.is_admin {
        return Ok(true);
    }
    let state = app_state().await?;
    let sites = role::employee_sites(&state.pool, person_id)
        .await
        .map_err(internal)?;
    Ok(scope_allows_sites(scope, &sites))
}

/// The signed-in, active person of this request. Sessions that were ended (devices revoked,
/// person deactivated, staff time limit) or whose passkey is revoked count as signed out.
pub async fn current_user() -> Result<SessionUser, ServerFnError> {
    let state = app_state().await?;
    session::current(&state.pool, &session().await?)
        .await
        .map_err(internal)?
        .ok_or_else(unauthorized)
}

async fn staff_scope_of(pool: &PgPool, user: &SessionUser) -> Result<StaffScope, ServerFnError> {
    if user.kind != SessionKind::Staff {
        return Err(forbidden());
    }
    let roles = role::roles_of(pool, user.person_id)
        .await
        .map_err(internal)?;
    scope_from_roles(user.person_id, &roles).ok_or_else(forbidden)
}

/// Manager of any site, or admin. Requires a staff session.
pub async fn require_staff() -> Result<StaffScope, ServerFnError> {
    let user = current_user().await?;
    let state = app_state().await?;
    staff_scope_of(&state.pool, &user).await
}

pub async fn require_admin() -> Result<StaffScope, ServerFnError> {
    let scope = require_staff().await?;
    if scope.is_admin {
        Ok(scope)
    } else {
        Err(forbidden())
    }
}

/// Manager of `site_id`; admins pass for every site.
pub async fn require_manager_of(site_id: Uuid) -> Result<StaffScope, ServerFnError> {
    let scope = require_staff().await?;
    if scope.is_admin || scope.site_ids.contains(&site_id) {
        Ok(scope)
    } else {
        Err(forbidden())
    }
}

/// HTTP-level defences that apply before any server function runs: security headers, a
/// cross-site request check on state-changing requests, rate limiting of `/api/auth/*` and
/// the per-request session expiry.
pub mod http {
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use axum::Router;
    use axum::extract::{Request, State};
    use axum::http::header::{
        CONTENT_LENGTH, CONTENT_TYPE, ORIGIN, RETRY_AFTER, STRICT_TRANSPORT_SECURITY,
    };
    use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
    use axum::middleware::{self, Next};
    use axum::response::{IntoResponse, Response};
    use tower_sessions::Session;

    use crate::server::config::AppConfig;
    use crate::server::session;

    const MSG_RATE_LIMITED: &str = "Te veel pogingen. Probeer het zo meteen opnieuw.";
    const MSG_CROSS_SITE: &str = "Aanvraag geweigerd.";
    /// Sign-in and enrollment endpoints; the session probe `me` is polled by the app and exempt.
    const RATE_LIMITED_PREFIX: &str = "/api/auth/";
    const RATE_LIMIT_EXEMPT: &str = "/api/auth/me";
    /// Above this many tracked clients, idle (full) buckets are dropped.
    const PRUNE_ABOVE: usize = 10_000;

    /// Content Security Policy for the Dioxus WASM app.
    ///
    /// - `script-src 'self' 'wasm-unsafe-eval'`: the bundle and WebAssembly compilation only.
    /// - `'unsafe-inline'` for scripts is needed because the Dioxus server renders inline
    ///   bootstrap scripts whose content changes per request (hydration data, streaming
    ///   resolvers) and offers no nonce hook, so hashes or a nonce are not feasible.
    /// - `style-src 'unsafe-inline'` covers `style` attributes set by the UI.
    const CSP: &str = "default-src 'self'; \
        script-src 'self' 'wasm-unsafe-eval' 'unsafe-inline'; \
        style-src 'self' 'unsafe-inline'; \
        img-src 'self' data: blob:; \
        font-src 'self'; \
        connect-src 'self'; \
        manifest-src 'self'; \
        worker-src 'self'; \
        object-src 'none'; \
        base-uri 'self'; \
        form-action 'self'; \
        frame-ancestors 'none'";
    const PERMISSIONS_POLICY: &str = "geolocation=(self), camera=(), microphone=()";
    const HSTS: &str = "max-age=31536000; includeSubDomains";

    /// Token bucket per client: `per_minute` burst, refilled evenly over a minute.
    pub struct RateLimiter {
        per_minute: f64,
        buckets: Mutex<HashMap<String, (f64, Instant)>>,
    }

    impl RateLimiter {
        pub fn new(per_minute: u32) -> Self {
            Self {
                per_minute: f64::from(per_minute),
                buckets: Mutex::new(HashMap::new()),
            }
        }

        /// Takes one token for `key` at time `now`; false when the bucket is empty.
        pub fn allow(&self, key: &str, now: Instant) -> bool {
            let rate = self.per_minute / 60.0;
            let refilled = |tokens: f64, last: Instant| {
                (tokens + now.saturating_duration_since(last).as_secs_f64() * rate)
                    .min(self.per_minute)
            };
            // A poisoned lock only means another request panicked; the map is still usable.
            let mut buckets = self
                .buckets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if buckets.len() >= PRUNE_ABOVE {
                buckets.retain(|_, (tokens, last)| refilled(*tokens, *last) < self.per_minute);
            }
            let (tokens, last) = buckets
                .entry(key.to_string())
                .or_insert((self.per_minute, now));
            let available = refilled(*tokens, *last);
            *last = now;
            if available >= 1.0 {
                *tokens = available - 1.0;
                true
            } else {
                *tokens = available;
                false
            }
        }
    }

    /// Shared state of the HTTP guards.
    #[derive(Clone)]
    pub struct HttpGuard {
        app_origin: Arc<str>,
        https: bool,
        trust_proxy_headers: bool,
        limiter: Arc<RateLimiter>,
    }

    impl HttpGuard {
        pub fn new(config: &AppConfig) -> Self {
            Self {
                app_origin: config.app_origin.as_str().into(),
                https: config.is_https(),
                trust_proxy_headers: config.trust_proxy_headers,
                limiter: Arc::new(RateLimiter::new(config.auth_rate_limit_per_minute)),
            }
        }

        /// Rate limit key of the request's client. Axum cannot see the peer address through
        /// `dioxus::serve`, so without a trusted proxy all clients share one bucket (a cap on
        /// the whole service, not per client). Behind a proxy the last `X-Forwarded-For`
        /// entry is the address the proxy itself saw; earlier entries are client-supplied.
        /// IPv6 clients are grouped by /64.
        fn client_key(&self, req: &Request) -> String {
            if !self.trust_proxy_headers {
                return "direct".to_string();
            }
            let forwarded = req
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok());
            match forwarded {
                Some(IpAddr::V6(ip)) => {
                    let s = ip.segments();
                    format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
                }
                Some(ip) => ip.to_string(),
                None => "unknown".to_string(),
            }
        }
    }

    /// Wraps the router with the guards. Outermost first: headers, cross-site check, rate limit.
    pub fn protect(router: Router, guard: HttpGuard) -> Router {
        router
            .layer(middleware::from_fn_with_state(guard.clone(), rate_limit))
            .layer(middleware::from_fn_with_state(
                guard.clone(),
                cross_site_check,
            ))
            .layer(middleware::from_fn_with_state(guard, security_headers))
    }

    async fn rate_limit(State(guard): State<HttpGuard>, req: Request, next: Next) -> Response {
        let path = req.uri().path();
        if path.starts_with(RATE_LIMITED_PREFIX)
            && path != RATE_LIMIT_EXEMPT
            && !guard.limiter.allow(&guard.client_key(&req), Instant::now())
        {
            tracing::warn!("auth rate limit hit");
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(RETRY_AFTER, "60")],
                MSG_RATE_LIMITED,
            )
                .into_response();
        }
        next.run(req).await
    }

    /// Defence in depth next to `SameSite=Lax`: a state-changing API request must not come from
    /// another origin and must not use a content type an HTML form can send.
    async fn cross_site_check(
        State(guard): State<HttpGuard>,
        req: Request,
        next: Next,
    ) -> Response {
        if req.uri().path().starts_with("/api/")
            && !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
        {
            let headers = req.headers();
            if let Some(origin) = headers.get(ORIGIN)
                && origin.as_bytes() != guard.app_origin.as_bytes()
            {
                tracing::warn!("state-changing request from a foreign origin refused");
                return (StatusCode::FORBIDDEN, MSG_CROSS_SITE).into_response();
            }
            // Server functions send JSON, or nothing at all for functions without arguments.
            let content_type_ok = match headers.get(CONTENT_TYPE) {
                Some(v) => v.to_str().is_ok_and(|v| {
                    v.split(';').next().unwrap_or_default().trim() == "application/json"
                }),
                None => headers
                    .get(CONTENT_LENGTH)
                    .is_none_or(|v| v.as_bytes() == b"0"),
            };
            if !content_type_ok {
                return (StatusCode::UNSUPPORTED_MEDIA_TYPE, MSG_CROSS_SITE).into_response();
            }
        }
        next.run(req).await
    }

    async fn security_headers(
        State(guard): State<HttpGuard>,
        req: Request,
        next: Next,
    ) -> Response {
        let mut res = next.run(req).await;
        let headers = res.headers_mut();
        let mut set = |name: &'static str, value: &'static str| {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
        };
        set("content-security-policy", CSP);
        set("x-frame-options", "DENY");
        set("referrer-policy", "no-referrer");
        set("x-content-type-options", "nosniff");
        set("permissions-policy", PERMISSIONS_POLICY);
        if guard.https {
            headers.insert(STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(HSTS));
        }
        res
    }

    /// Applies the right session expiry to every API request (see `session` module).
    pub async fn refresh_session_expiry(req: Request, next: Next) -> Response {
        if req.uri().path().starts_with("/api/")
            && let Some(session) = req.extensions().get::<Session>()
        {
            session::refresh_expiry(session).await;
        }
        next.run(req).await
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;
        use std::time::Duration;

        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use axum::routing::{get, post};
        use tower::ServiceExt;

        use super::*;

        fn guard(origin: &str, trust: bool, per_minute: u32) -> HttpGuard {
            let env: HashMap<String, String> = [
                ("DATABASE_URL", "postgres://unused"),
                ("APP_ORIGIN", origin),
                ("DATA_KEY_FILE", "/unused"),
                ("BLIND_INDEX_KEY_FILE", "/unused"),
                (
                    "COOKIE_SECURE",
                    if origin.starts_with("https") {
                        "true"
                    } else {
                        "false"
                    },
                ),
                ("TRUST_PROXY_HEADERS", if trust { "true" } else { "false" }),
                ("AUTH_RATE_LIMIT_PER_MINUTE", &per_minute.to_string()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            HttpGuard::new(&AppConfig::from_map(&env).unwrap())
        }

        fn app(guard: HttpGuard) -> Router {
            protect(
                Router::new()
                    .route("/", get(|| async { "page" }))
                    .route("/api/auth/login/begin", post(|| async { "ok" }))
                    .route("/api/auth/me", get(|| async { "me" }))
                    .route("/api/clock", post(|| async { "ok" })),
                guard,
            )
        }

        async fn send(
            app: &Router,
            method: &str,
            path: &str,
            headers: &[(&str, &str)],
        ) -> Response {
            let mut req = HttpRequest::builder().method(method).uri(path);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            app.clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap()
        }

        #[tokio::test]
        async fn token_bucket_refills_over_time() {
            let l = RateLimiter::new(6);
            let t0 = Instant::now();
            for _ in 0..6 {
                assert!(l.allow("a", t0));
            }
            assert!(!l.allow("a", t0));
            assert!(l.allow("b", t0), "other clients are independent");
            assert!(!l.allow("a", t0 + Duration::from_secs(5)));
            assert!(
                l.allow("a", t0 + Duration::from_secs(11)),
                "one token per 10 s"
            );
            assert!(!l.allow("a", t0 + Duration::from_secs(12)));
            // Never more than the burst, however long the pause.
            let later = t0 + Duration::from_secs(3600);
            for _ in 0..6 {
                assert!(l.allow("a", later));
            }
            assert!(!l.allow("a", later));
        }

        #[tokio::test]
        async fn auth_endpoints_are_rate_limited_with_a_generic_429() {
            let app = app(guard("https://tijd.example.org", true, 3));
            let ip = [("x-forwarded-for", "203.0.113.7")];
            for _ in 0..3 {
                let r = send(&app, "POST", "/api/auth/login/begin", &ip).await;
                assert_eq!(r.status(), StatusCode::OK);
            }
            let r = send(&app, "POST", "/api/auth/login/begin", &ip).await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(r.headers()[RETRY_AFTER], "60");
            assert!(r.headers().contains_key("content-security-policy"));
            // Another client, the session probe and other endpoints are unaffected.
            let other = [("x-forwarded-for", "203.0.113.8")];
            assert_eq!(
                send(&app, "POST", "/api/auth/login/begin", &other)
                    .await
                    .status(),
                StatusCode::OK
            );
            for _ in 0..5 {
                assert_eq!(
                    send(&app, "GET", "/api/auth/me", &ip).await.status(),
                    StatusCode::OK
                );
                assert_eq!(
                    send(&app, "POST", "/api/clock", &ip).await.status(),
                    StatusCode::OK
                );
            }
        }

        #[tokio::test]
        async fn forwarded_header_is_ignored_unless_trusted_and_last_entry_counts() {
            // Untrusted: one shared bucket, spoofing changes nothing.
            let app_direct = app(guard("https://tijd.example.org", false, 2));
            for ip in ["198.51.100.1", "198.51.100.2"] {
                send(
                    &app_direct,
                    "POST",
                    "/api/auth/login/begin",
                    &[("x-forwarded-for", ip)],
                )
                .await;
            }
            let r = send(
                &app_direct,
                "POST",
                "/api/auth/login/begin",
                &[("x-forwarded-for", "198.51.100.3")],
            )
            .await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
            // Trusted: the client cannot pick its bucket by prepending addresses.
            let app_proxy = app(guard("https://tijd.example.org", true, 2));
            for spoof in ["1.1.1.1", "2.2.2.2"] {
                let v = format!("{spoof}, 203.0.113.9");
                send(
                    &app_proxy,
                    "POST",
                    "/api/auth/login/begin",
                    &[("x-forwarded-for", &v)],
                )
                .await;
            }
            let r = send(
                &app_proxy,
                "POST",
                "/api/auth/login/begin",
                &[("x-forwarded-for", "3.3.3.3, 203.0.113.9")],
            )
            .await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        }

        #[tokio::test]
        async fn ipv6_clients_share_a_bucket_per_64() {
            let app = app(guard("https://tijd.example.org", true, 1));
            let a = [("x-forwarded-for", "2001:db8:1:2::1")];
            let b = [("x-forwarded-for", "2001:db8:1:2:ffff::9")];
            assert_eq!(
                send(&app, "POST", "/api/auth/login/begin", &a)
                    .await
                    .status(),
                StatusCode::OK
            );
            assert_eq!(
                send(&app, "POST", "/api/auth/login/begin", &b)
                    .await
                    .status(),
                StatusCode::TOO_MANY_REQUESTS
            );
        }

        #[tokio::test]
        async fn security_headers_on_every_response() {
            let https = app(guard("https://tijd.example.org", false, 10));
            let r = send(&https, "GET", "/", &[]).await;
            let h = r.headers();
            let csp = h["content-security-policy"].to_str().unwrap();
            assert!(csp.contains("default-src 'self'"));
            assert!(csp.contains("frame-ancestors 'none'"));
            assert!(csp.contains("'wasm-unsafe-eval'"));
            assert_eq!(h["x-frame-options"], "DENY");
            assert_eq!(h["referrer-policy"], "no-referrer");
            assert_eq!(h["x-content-type-options"], "nosniff");
            assert_eq!(
                h["permissions-policy"],
                "geolocation=(self), camera=(), microphone=()"
            );
            assert_eq!(h[STRICT_TRANSPORT_SECURITY], HSTS);
            // Also on errors, and no HSTS over plain http.
            let http = app(guard("http://localhost:8080", false, 10));
            let r = send(&http, "GET", "/nope", &[]).await;
            assert_eq!(r.status(), StatusCode::NOT_FOUND);
            assert!(r.headers().contains_key("x-frame-options"));
            assert!(!r.headers().contains_key(STRICT_TRANSPORT_SECURITY));
        }

        #[tokio::test]
        async fn foreign_origin_and_form_posts_are_refused() {
            let app = app(guard("https://tijd.example.org", false, 100));
            let json = ("content-type", "application/json");
            let ok = ("origin", "https://tijd.example.org");
            assert_eq!(
                send(&app, "POST", "/api/clock", &[ok, json]).await.status(),
                StatusCode::OK
            );
            // No Origin (same-origin tools, older browsers), JSON with a charset, no body at all.
            assert_eq!(
                send(&app, "POST", "/api/clock", &[json]).await.status(),
                StatusCode::OK
            );
            assert_eq!(
                send(
                    &app,
                    "POST",
                    "/api/clock",
                    &[("content-type", "application/json; charset=utf-8")]
                )
                .await
                .status(),
                StatusCode::OK
            );
            assert_eq!(
                send(&app, "POST", "/api/clock", &[]).await.status(),
                StatusCode::OK
            );
            for bad in [
                "https://evil.example.com",
                "null",
                "http://tijd.example.org",
                "https://tijd.example.org.evil.example",
            ] {
                let r = send(&app, "POST", "/api/clock", &[("origin", bad), json]).await;
                assert_eq!(r.status(), StatusCode::FORBIDDEN, "origin {bad}");
            }
            for bad in [
                "application/x-www-form-urlencoded",
                "text/plain",
                "multipart/form-data; boundary=x",
            ] {
                let r = send(&app, "POST", "/api/clock", &[ok, ("content-type", bad)]).await;
                assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE, "type {bad}");
            }
            // A body without a content type is not something a server function client sends.
            let r = send(&app, "POST", "/api/clock", &[("content-length", "5")]).await;
            assert_eq!(r.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
            // Reads are not subject to the check.
            let r = send(
                &app,
                "GET",
                "/api/auth/me",
                &[("origin", "https://evil.example.com")],
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
        }
    }
}
