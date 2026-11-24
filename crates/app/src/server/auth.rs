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
    if scope.can_view_all_sites() {
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

/// Manager of any site, HR or admin. Requires a staff session.
pub async fn require_staff() -> Result<StaffScope, ServerFnError> {
    let user = current_user().await?;
    let state = app_state().await?;
    staff_scope_of(&state.pool, &user).await
}

pub async fn require_admin() -> Result<StaffScope, ServerFnError> {
    let state = app_state().await?;
    admin_scope(&state.pool, &session().await?).await
}

/// The admin behind `session`; everyone else is refused.
pub async fn admin_scope(
    pool: &PgPool,
    session: &tower_sessions::Session,
) -> Result<StaffScope, ServerFnError> {
    let user = session::current(pool, session)
        .await
        .map_err(internal)?
        .ok_or_else(unauthorized)?;
    admin_scope_of(pool, &user).await
}

async fn admin_scope_of(pool: &PgPool, user: &SessionUser) -> Result<StaffScope, ServerFnError> {
    let scope = staff_scope_of(pool, user).await?;
    if scope.can_manage_settings() {
        Ok(scope)
    } else {
        Err(forbidden())
    }
}

/// An admin, or, while the installation is still in setup mode, an anonymous session that
/// already entered a valid setup code. Anyone else is refused.
pub async fn require_admin_or_setup(
    pool: &PgPool,
    session: &tower_sessions::Session,
) -> Result<(), ServerFnError> {
    if let Some(user) = session::current(pool, session).await.map_err(internal)? {
        return admin_scope_of(pool, &user).await.map(drop);
    }
    if session::setup_code_ok(session).await
        && store::repo::setup::setup_required(pool)
            .await
            .map_err(internal)?
    {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

/// Manager of `site_id`; admins and HR pass for every site.
pub async fn require_manager_of(site_id: Uuid) -> Result<StaffScope, ServerFnError> {
    let scope = require_staff().await?;
    if scope.can_view_all_sites() || scope.site_ids.contains(&site_id) {
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
    use std::net::{IpAddr, SocketAddr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use axum::Router;
    use axum::extract::{ConnectInfo, Request, State};
    use axum::http::header::{
        CONTENT_LENGTH, CONTENT_TYPE, ORIGIN, RETRY_AFTER, STRICT_TRANSPORT_SECURITY,
        TRANSFER_ENCODING,
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
    const AUTH_PREFIX: &str = "/api/auth/";
    const AUTH_EXEMPT: &str = "/api/auth/me";
    /// The setup wizard endpoints: strict, they are only used a few times per installation.
    const SETUP_PREFIX: &str = "/api/setup/";
    const SETUP_PER_MINUTE: u32 = 10;
    /// Public reads every page load makes: generous, a school shares one address.
    const PUBLIC_READS: [&str; 3] = [
        "/api/setup/required",
        "/api/branding",
        "/api/geocode/enabled",
    ];
    const PUBLIC_PER_MINUTE: u32 = 120;
    /// Address search: every call may cost an upstream request, which is also limited globally.
    const GEOCODE_PATH: &str = "/api/geocode";
    const GEOCODE_PER_MINUTE: u32 = 30;
    /// Ceremony endpoints whose failures (400/401) are counted per client.
    const FINISH_PATHS: [&str; 2] = ["/api/auth/login/finish", "/api/auth/register/finish"];
    const FAILURES_PER_WINDOW: u32 = 10;
    const FAILURE_WINDOW: Duration = Duration::from_secs(600);
    /// Clocking in or out per authenticated person (not per address).
    const CLOCK_PATH: &str = "/api/clock";
    const CLOCK_PER_MINUTE: u32 = 20;
    /// Hard cap on tracked keys per limiter; the map is also pruned periodically.
    const MAX_TRACKED: usize = 10_000;
    /// Safety cap for all auth requests together, used when no client address is known.
    const GLOBAL_CAP_PER_MINUTE: u32 = 600;

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

    struct Buckets {
        map: HashMap<String, (f64, Instant)>,
        last_prune: Option<Instant>,
    }

    /// Token bucket per key: `capacity` burst, refilled evenly over `window`.
    ///
    /// Memory is bounded: once per `window` all idle (full) buckets are dropped in one pass, and
    /// at [`MAX_TRACKED`] keys an arbitrary other key is evicted per new key, so no request ever
    /// scans the whole map.
    pub struct RateLimiter {
        capacity: f64,
        window: Duration,
        buckets: Mutex<Buckets>,
    }

    impl RateLimiter {
        /// `per_minute` requests per minute.
        pub fn new(per_minute: u32) -> Self {
            Self::with_window(per_minute, Duration::from_secs(60))
        }

        /// `capacity` requests per `window`.
        pub fn with_window(capacity: u32, window: Duration) -> Self {
            Self {
                capacity: f64::from(capacity),
                window,
                buckets: Mutex::new(Buckets {
                    map: HashMap::new(),
                    last_prune: None,
                }),
            }
        }

        fn refilled(&self, tokens: f64, last: Instant, now: Instant) -> f64 {
            let rate = self.capacity / self.window.as_secs_f64();
            (tokens + now.saturating_duration_since(last).as_secs_f64() * rate).min(self.capacity)
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Buckets> {
            // A poisoned lock only means another request panicked; the map is still usable.
            self.buckets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// Number of tracked keys.
        pub fn tracked(&self) -> usize {
            self.lock().map.len()
        }

        /// Whether `key` has a token left at `now`, without taking one.
        pub fn has_capacity(&self, key: &str, now: Instant) -> bool {
            let buckets = self.lock();
            buckets
                .map
                .get(key)
                .is_none_or(|(tokens, last)| self.refilled(*tokens, *last, now) >= 1.0)
        }

        /// Takes one token for `key` at time `now`; false when the bucket is empty.
        pub fn allow(&self, key: &str, now: Instant) -> bool {
            let mut buckets = self.lock();
            let last_prune = *buckets.last_prune.get_or_insert(now);
            if now.saturating_duration_since(last_prune) >= self.window {
                buckets.last_prune = Some(now);
                buckets
                    .map
                    .retain(|_, (tokens, last)| self.refilled(*tokens, *last, now) < self.capacity);
            }
            if buckets.map.len() >= MAX_TRACKED && !buckets.map.contains_key(key) {
                let evict = buckets.map.keys().next().cloned();
                if let Some(evict) = evict {
                    buckets.map.remove(&evict);
                }
            }
            let (tokens, last) = buckets
                .map
                .entry(key.to_string())
                .or_insert((self.capacity, now));
            let available = self.refilled(*tokens, *last, now);
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

    /// Which limiter a request path falls under.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Class {
        Auth,
        Setup,
        Public,
        Geocode,
    }

    impl Class {
        fn of(path: &str) -> Option<Self> {
            if PUBLIC_READS.contains(&path) {
                Some(Self::Public)
            } else if path == GEOCODE_PATH {
                Some(Self::Geocode)
            } else if path.starts_with(SETUP_PREFIX) {
                Some(Self::Setup)
            } else if path.starts_with(AUTH_PREFIX) && path != AUTH_EXEMPT {
                Some(Self::Auth)
            } else {
                None
            }
        }

        fn name(self) -> &'static str {
            match self {
                Self::Auth => "auth",
                Self::Setup => "setup",
                Self::Public => "public",
                Self::Geocode => "geocode",
            }
        }
    }

    /// Shared state of the HTTP guards.
    #[derive(Clone)]
    pub struct HttpGuard {
        app_origin: Arc<str>,
        https: bool,
        trust_proxy_headers: bool,
        auth: Arc<RateLimiter>,
        setup: Arc<RateLimiter>,
        public: Arc<RateLimiter>,
        geocode: Arc<RateLimiter>,
        failures: Arc<RateLimiter>,
        clock: Arc<RateLimiter>,
        global: Arc<RateLimiter>,
        warned_no_peer: Arc<AtomicBool>,
    }

    impl HttpGuard {
        pub fn new(config: &AppConfig) -> Self {
            Self {
                app_origin: config.app_origin.as_str().into(),
                https: config.is_https(),
                trust_proxy_headers: config.trust_proxy_headers,
                auth: Arc::new(RateLimiter::new(config.auth_rate_limit_per_minute)),
                setup: Arc::new(RateLimiter::new(SETUP_PER_MINUTE)),
                public: Arc::new(RateLimiter::new(PUBLIC_PER_MINUTE)),
                geocode: Arc::new(RateLimiter::new(GEOCODE_PER_MINUTE)),
                failures: Arc::new(RateLimiter::with_window(
                    FAILURES_PER_WINDOW,
                    FAILURE_WINDOW,
                )),
                clock: Arc::new(RateLimiter::new(CLOCK_PER_MINUTE)),
                global: Arc::new(RateLimiter::new(GLOBAL_CAP_PER_MINUTE)),
                warned_no_peer: Arc::new(AtomicBool::new(false)),
            }
        }

        /// Rate limit key of the request's client, `None` when no address is known.
        ///
        /// The peer address comes from axum's `ConnectInfo` (the server is started with
        /// `into_make_service_with_connect_info`, see `server::serve`). Behind a trusted proxy
        /// the last entry of the last `X-Forwarded-For` header line is the address the proxy
        /// itself saw; everything before it is client-supplied. Without trusted proxy headers
        /// the header is ignored. IPv4-mapped IPv6 addresses count as their IPv4 address and
        /// IPv6 clients are grouped by /64.
        fn client_key(&self, req: &Request) -> Option<String> {
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip());
            let ip = if self.trust_proxy_headers {
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
            Some(match ip?.to_canonical() {
                IpAddr::V6(ip) => {
                    let s = ip.segments();
                    format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
                }
                ip => ip.to_string(),
            })
        }

        fn limiter(&self, class: Class) -> &RateLimiter {
            match class {
                Class::Auth => &self.auth,
                Class::Setup => &self.setup,
                Class::Public => &self.public,
                Class::Geocode => &self.geocode,
            }
        }

        /// Takes a token for this request: from the client's own bucket, or from the global
        /// safety cap when the client cannot be told apart (no peer address available).
        fn allow(&self, class: Class, key: Option<&str>, now: Instant) -> bool {
            match key {
                Some(key) => self.limiter(class).allow(key, now),
                None => {
                    if !self.warned_no_peer.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            "client address unknown: per-client rate limiting is off and only a \
                             global cap applies; run behind a reverse proxy with \
                             TRUST_PROXY_HEADERS=true"
                        );
                    }
                    self.global.allow(class.name(), now)
                }
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

    fn too_many_requests() -> Response {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(RETRY_AFTER, "60")],
            MSG_RATE_LIMITED,
        )
            .into_response()
    }

    async fn rate_limit(State(guard): State<HttpGuard>, req: Request, next: Next) -> Response {
        let path = req.uri().path().to_string();
        let Some(class) = Class::of(&path) else {
            return next.run(req).await;
        };
        let now = Instant::now();
        let key = guard.client_key(&req);
        let finish = FINISH_PATHS.contains(&path.as_str());
        // A client with too many failed attempts is stopped before anything is verified.
        if finish
            && key
                .as_deref()
                .is_some_and(|k| !guard.failures.has_capacity(k, now))
        {
            tracing::warn!("too many failed sign-in attempts from a client");
            return too_many_requests();
        }
        if !guard.allow(class, key.as_deref(), now) {
            tracing::warn!(class = class.name(), "rate limit hit");
            return too_many_requests();
        }
        let res = next.run(req).await;
        if finish
            && matches!(
                res.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
            )
            && let Some(key) = key
        {
            guard.failures.allow(&key, Instant::now());
        }
        res
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
                // No body may follow: neither a chunked one nor a non-empty `Content-Length`.
                None => {
                    !headers.contains_key(TRANSFER_ENCODING)
                        && headers.get(CONTENT_LENGTH).is_none_or(|v| {
                            v.to_str()
                                .ok()
                                .and_then(|v| v.trim().parse::<u64>().ok())
                                .is_some_and(|n| n == 0)
                        })
                }
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

    /// Adds the session layer and, inside it, the session guard: the role-based session expiry
    /// on every request (pages and assets included, so a page load never shortens a signed-in
    /// session to the anonymous default) and a throttle on clocking per signed-in person.
    pub fn with_session<S: tower_sessions::SessionStore + Clone>(
        router: Router,
        layer: tower_sessions::SessionManagerLayer<S>,
        guard: HttpGuard,
    ) -> Router {
        router
            .layer(middleware::from_fn_with_state(guard, session_guard))
            .layer(layer)
    }

    async fn session_guard(State(guard): State<HttpGuard>, req: Request, next: Next) -> Response {
        let user = match req.extensions().get::<Session>() {
            Some(session) => session::refresh_expiry(session).await,
            None => None,
        };
        // Keyed on the person (cookie holder), so many devices or one shared address do not
        // change the limit. Sessions that turn out invalid are refused further on anyway.
        if req.uri().path() == CLOCK_PATH
            && *req.method() == Method::POST
            && let Some(user) = user
            && !guard
                .clock
                .allow(&user.person_id.to_string(), Instant::now())
        {
            tracing::warn!("clock throttle hit");
            return too_many_requests();
        }
        next.run(req).await
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;
        use std::time::Duration;

        use axum::body::Body;
        use axum::extract::ConnectInfo;
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

        /// Answers with the status named in `x-test-status`, like a failing or passing ceremony.
        async fn finish(headers: axum::http::HeaderMap) -> (StatusCode, &'static str) {
            let code = headers
                .get("x-test-status")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(200);
            (StatusCode::from_u16(code).unwrap(), "done")
        }

        fn app(guard: HttpGuard) -> Router {
            protect(
                Router::new()
                    .route("/", get(|| async { "page" }))
                    .route("/api/auth/login/begin", post(|| async { "ok" }))
                    .route("/api/auth/me", get(|| async { "me" }))
                    .route("/api/clock", post(|| async { "ok" }))
                    .route("/api/branding", get(|| async { "branding" }))
                    .route("/api/setup/required", get(|| async { "required" }))
                    .route("/api/setup/check-code", post(|| async { "ok" }))
                    .route("/api/setup/complete", post(|| async { "ok" }))
                    .route("/api/auth/login/finish", post(finish))
                    .route("/api/auth/register/finish", post(finish)),
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

        /// Like `send`, with the peer address that `into_make_service_with_connect_info` adds.
        async fn send_from(
            app: &Router,
            peer: &str,
            path: &str,
            headers: &[(&str, &str)],
        ) -> StatusCode {
            let mut req = HttpRequest::builder().method("POST").uri(path);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            let mut req = req.body(Body::empty()).unwrap();
            let addr: std::net::SocketAddr = format!("{peer}:4000").parse().unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
            app.clone().oneshot(req).await.unwrap().status()
        }

        const LOGIN: &str = "/api/auth/login/begin";

        #[tokio::test]
        async fn peer_addresses_get_separate_buckets_without_trusted_proxy() {
            let app = app(guard("https://tijd.example.org", false, 2));
            for _ in 0..2 {
                assert_eq!(
                    send_from(&app, "198.51.100.1", LOGIN, &[]).await,
                    StatusCode::OK
                );
            }
            assert_eq!(
                send_from(&app, "198.51.100.1", LOGIN, &[]).await,
                StatusCode::TOO_MANY_REQUESTS
            );
            // Another employee on another address is not locked out.
            assert_eq!(
                send_from(&app, "198.51.100.2", LOGIN, &[]).await,
                StatusCode::OK
            );
            // A spoofed forwarding header changes nothing without a trusted proxy.
            let spoof = [("x-forwarded-for", "203.0.113.50")];
            assert_eq!(
                send_from(&app, "198.51.100.1", LOGIN, &spoof).await,
                StatusCode::TOO_MANY_REQUESTS
            );
        }

        #[tokio::test]
        async fn trusted_proxy_keys_on_forwarded_entry_and_falls_back_to_peer() {
            let app = app(guard("https://tijd.example.org", true, 1));
            let a = [("x-forwarded-for", "203.0.113.7")];
            let b = [("x-forwarded-for", "203.0.113.8")];
            assert_eq!(send_from(&app, "10.0.0.1", LOGIN, &a).await, StatusCode::OK);
            assert_eq!(send_from(&app, "10.0.0.1", LOGIN, &b).await, StatusCode::OK);
            assert_eq!(
                send_from(&app, "10.0.0.1", LOGIN, &a).await,
                StatusCode::TOO_MANY_REQUESTS
            );
            // No header: the peer address is the key.
            assert_eq!(
                send_from(&app, "10.0.0.2", LOGIN, &[]).await,
                StatusCode::OK
            );
            assert_eq!(
                send_from(&app, "10.0.0.2", LOGIN, &[]).await,
                StatusCode::TOO_MANY_REQUESTS
            );
        }

        #[tokio::test]
        async fn without_peer_address_only_the_global_cap_applies() {
            // Per-client limit of 1, yet many requests pass: no usable client key.
            let app = app(guard("https://tijd.example.org", false, 1));
            let mut passed = 0;
            for _ in 0..GLOBAL_CAP_PER_MINUTE * 2 {
                if send(&app, "POST", LOGIN, &[]).await.status() == StatusCode::OK {
                    passed += 1;
                }
            }
            // The cap, plus what refills while the loop runs.
            assert!((GLOBAL_CAP_PER_MINUTE..GLOBAL_CAP_PER_MINUTE + 50).contains(&passed));
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
        async fn last_forwarded_entry_counts_when_trusted() {
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

        async fn send_peer(
            app: &Router,
            method: &str,
            peer: std::net::IpAddr,
            path: &str,
            headers: &[(&str, &str)],
        ) -> StatusCode {
            let mut req = HttpRequest::builder().method(method).uri(path);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            let mut req = req.body(Body::empty()).unwrap();
            req.extensions_mut()
                .insert(ConnectInfo(std::net::SocketAddr::new(peer, 4000)));
            app.clone().oneshot(req).await.unwrap().status()
        }

        #[tokio::test]
        async fn forwarded_for_uses_the_last_value_across_all_header_lines() {
            let app = app(guard("https://tijd.example.org", true, 1));
            let line = |first: &'static str| {
                [
                    ("x-forwarded-for", first),
                    ("x-forwarded-for", "203.0.113.9"),
                ]
            };
            assert_eq!(
                send(&app, "POST", LOGIN, &line("7.7.7.7")).await.status(),
                StatusCode::OK
            );
            // A second header line is the proxy's: the client cannot choose another bucket.
            assert_eq!(
                send(&app, "POST", LOGIN, &line("8.8.8.8")).await.status(),
                StatusCode::TOO_MANY_REQUESTS
            );
            // The proxy's own entry in the last line decides, also within one line.
            let r = send(
                &app,
                "POST",
                LOGIN,
                &[
                    ("x-forwarded-for", "1.1.1.1"),
                    ("x-forwarded-for", "2.2.2.2, 203.0.113.9"),
                ],
            )
            .await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        }

        #[tokio::test]
        async fn ipv4_mapped_ipv6_shares_the_bucket_of_the_ipv4_address() {
            let app = app(guard("https://tijd.example.org", false, 1));
            let v4: std::net::IpAddr = "198.51.100.1".parse().unwrap();
            let mapped: std::net::IpAddr = "::ffff:198.51.100.1".parse().unwrap();
            assert_eq!(
                send_peer(&app, "POST", v4, LOGIN, &[]).await,
                StatusCode::OK
            );
            assert_eq!(
                send_peer(&app, "POST", mapped, LOGIN, &[]).await,
                StatusCode::TOO_MANY_REQUESTS
            );
            let proxied = app_with_proxy_trust();
            let a = [("x-forwarded-for", "203.0.113.7")];
            let b = [("x-forwarded-for", "::ffff:203.0.113.7")];
            assert_eq!(
                send(&proxied, "POST", LOGIN, &a).await.status(),
                StatusCode::OK
            );
            assert_eq!(
                send(&proxied, "POST", LOGIN, &b).await.status(),
                StatusCode::TOO_MANY_REQUESTS
            );
        }

        fn app_with_proxy_trust() -> Router {
            app(guard("https://tijd.example.org", true, 1))
        }

        #[tokio::test]
        async fn setup_endpoints_are_limited_strictly_and_public_reads_generously() {
            let app = app(guard("https://tijd.example.org", true, 1000));
            let ip = [("x-forwarded-for", "203.0.113.7")];
            for _ in 0..10 {
                let r = send(&app, "POST", "/api/setup/check-code", &ip).await;
                assert_eq!(r.status(), StatusCode::OK);
            }
            let r = send(&app, "POST", "/api/setup/check-code", &ip).await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(r.headers()[RETRY_AFTER], "60");
            // Same bucket for the other strict setup endpoint.
            let r = send(&app, "POST", "/api/setup/complete", &ip).await;
            assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
            // Another client is unaffected.
            let other = [("x-forwarded-for", "203.0.113.8")];
            let r = send(&app, "POST", "/api/setup/check-code", &other).await;
            assert_eq!(r.status(), StatusCode::OK);
            // The two public reads: generous, so a school behind one NAT is fine.
            // (They share one bucket per client, so each path is tried with its own client.)
            for (path, client) in [
                ("/api/setup/required", "203.0.113.20"),
                ("/api/branding", "203.0.113.21"),
            ] {
                let ip = [("x-forwarded-for", client)];
                for _ in 0..120 {
                    assert_eq!(send(&app, "GET", path, &ip).await.status(), StatusCode::OK);
                }
                let r = send(&app, "GET", path, &ip).await;
                assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            }
        }

        #[tokio::test]
        async fn failed_finish_attempts_are_counted_per_client() {
            let app = app(guard("https://tijd.example.org", true, 1000));
            let ip = |status: &'static str| {
                [
                    ("x-forwarded-for", "203.0.113.7"),
                    ("x-test-status", status),
                ]
            };
            // Successes do not count, however many.
            for _ in 0..30 {
                let r = send(&app, "POST", "/api/auth/login/finish", &ip("200")).await;
                assert_eq!(r.status(), StatusCode::OK);
            }
            // Ten failures (401 and 400 both count, across both finish endpoints) ...
            for i in 0..10 {
                let (path, status) = if i % 2 == 0 {
                    ("/api/auth/login/finish", "401")
                } else {
                    ("/api/auth/register/finish", "400")
                };
                let r = send(&app, "POST", path, &ip(status)).await;
                assert_eq!(r.status().as_u16(), status.parse::<u16>().unwrap());
            }
            // ... and the client is stopped, even with a correct attempt.
            for path in ["/api/auth/login/finish", "/api/auth/register/finish"] {
                let r = send(&app, "POST", path, &ip("200")).await;
                assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
                assert_eq!(r.headers()[RETRY_AFTER], "60");
            }
            // Beginning a ceremony and other clients keep working.
            let r = send(&app, "POST", LOGIN, &ip("200")).await;
            assert_eq!(r.status(), StatusCode::OK);
            let other = [("x-forwarded-for", "203.0.113.8"), ("x-test-status", "200")];
            let r = send(&app, "POST", "/api/auth/login/finish", &other).await;
            assert_eq!(r.status(), StatusCode::OK);
        }

        #[test]
        fn failure_limiter_refills_over_ten_minutes() {
            let l = RateLimiter::with_window(10, Duration::from_secs(600));
            let t0 = Instant::now();
            for _ in 0..10 {
                assert!(l.allow("a", t0));
            }
            assert!(!l.has_capacity("a", t0));
            assert!(!l.has_capacity("a", t0 + Duration::from_secs(50)));
            assert!(l.has_capacity("a", t0 + Duration::from_secs(61)));
        }

        #[test]
        fn tracked_clients_stay_bounded_without_scanning_on_every_request() {
            let l = RateLimiter::new(5);
            let t0 = Instant::now();
            for i in 0..MAX_TRACKED * 3 {
                // Every client holds a partly used bucket, so none counts as idle.
                assert!(l.allow(&format!("client-{i}"), t0));
            }
            assert!(l.tracked() <= MAX_TRACKED, "{} tracked", l.tracked());
            // Idle clients are dropped on the next periodic prune.
            let later = t0 + Duration::from_secs(3600);
            assert!(l.allow("fresh", later));
            assert_eq!(l.tracked(), 1);
        }

        #[tokio::test]
        async fn bodies_without_a_content_type_are_refused() {
            let app = app(guard("https://tijd.example.org", false, 100));
            for headers in [
                &[("transfer-encoding", "chunked")][..],
                &[("content-length", "1")][..],
                &[("content-length", "nonsense")][..],
            ] {
                let r = send(&app, "POST", "/api/clock", headers).await;
                assert_eq!(
                    r.status(),
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "{headers:?}"
                );
            }
            let r = send(&app, "POST", "/api/clock", &[("content-length", "0")]).await;
            assert_eq!(r.status(), StatusCode::OK);
        }

        // --- session layer: expiry on every request, per-person clock throttle ---

        use tower_sessions::{Expiry, MemoryStore};

        use crate::server::session::{SessionKind, SessionUser};

        fn person(n: u128) -> uuid::Uuid {
            uuid::Uuid::from_u128(n)
        }

        fn session_app() -> Router {
            let guard = guard("https://tijd.example.org", false, 1000);
            let layer = session::base_layer(MemoryStore::default(), "tr_session", false);
            let router = Router::new()
                .route("/", get(|| async { "page" }))
                .route("/api/clock", post(|| async { "ok" }))
                .route("/api/clock/today", get(|| async { "today" }))
                .route(
                    "/login/{n}",
                    get(
                        |session: Session, axum::extract::Path(n): axum::extract::Path<u128>| async move {
                            session::set_user(
                                &session,
                                SessionUser {
                                    person_id: person(n),
                                    kind: SessionKind::Employee,
                                    authenticated_at: chrono::Utc::now(),
                                    passkey_id: uuid::Uuid::new_v4(),
                                },
                            )
                            .await
                            .unwrap();
                            "in"
                        },
                    ),
                );
            with_session(router, layer, guard)
        }

        fn cookie_of(res: &Response) -> Option<String> {
            res.headers()
                .get("set-cookie")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(';').next().unwrap_or_default().to_string())
        }

        fn max_age_of(res: &Response) -> Option<i64> {
            let v = res.headers().get("set-cookie")?.to_str().ok()?;
            let (_, rest) = v.split_once("Max-Age=")?;
            rest.split(';').next()?.trim().parse().ok()
        }

        async fn login(app: &Router, n: u128) -> String {
            let r = send(app, "GET", &format!("/login/{n}"), &[]).await;
            cookie_of(&r).expect("session cookie")
        }

        #[tokio::test]
        async fn page_loads_keep_the_role_based_session_expiry() {
            let app = session_app();
            let cookie = login(&app, 1).await;
            let thirty_days = 30 * 24 * 3600;
            for path in ["/", "/api/clock/today", "/missing"] {
                let r = send(&app, "GET", path, &[("cookie", &cookie)]).await;
                let age = max_age_of(&r).unwrap_or_else(|| panic!("no Max-Age on {path}"));
                assert!(
                    (thirty_days - 120..=thirty_days).contains(&age),
                    "{path}: {age}s"
                );
            }
            // Anonymous visitors still get the short lifetime.
            let r = send(&app, "GET", "/", &[]).await;
            if let Some(age) = max_age_of(&r) {
                assert!(age <= 600, "{age}s");
            }
            let _ = Expiry::OnSessionEnd;
        }

        #[tokio::test]
        async fn clock_is_throttled_per_person_not_per_address() {
            let app = session_app();
            let (a, b) = (login(&app, 1).await, login(&app, 2).await);
            let post = |cookie: &str, peer: &str| {
                let app = app.clone();
                let cookie = cookie.to_string();
                let peer: std::net::IpAddr = peer.parse().unwrap();
                async move { send_peer(&app, "POST", peer, "/api/clock", &[("cookie", &cookie)]).await }
            };
            for i in 0..20 {
                // Different addresses (school wifi, mobile data) do not help.
                let peer = format!("198.51.100.{}", i + 1);
                assert_eq!(post(&a, &peer).await, StatusCode::OK, "call {i}");
            }
            assert_eq!(
                post(&a, "198.51.100.99").await,
                StatusCode::TOO_MANY_REQUESTS
            );
            // A colleague on the same address is not affected.
            assert_eq!(post(&b, "198.51.100.1").await, StatusCode::OK);
            // Reading today's state is not throttled this way, and neither are anonymous calls.
            let r = send(&app, "GET", "/api/clock/today", &[("cookie", &a)]).await;
            assert_eq!(r.status(), StatusCode::OK);
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
