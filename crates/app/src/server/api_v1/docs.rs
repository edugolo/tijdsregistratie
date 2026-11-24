//! The interactive API reference (Scalar) and the OpenAPI document. Off unless `API_DOCS=on`.
//!
//! The Scalar standalone script is vendored (`crates/app/vendor/scalar/`, see its README) and
//! embedded in the binary, so it is served from this origin. The page has no inline script: its
//! configuration sits in a `data-configuration` attribute. It carries its own Content Security
//! Policy ([`CSP`]), stricter than the app's: no inline script and no `eval`, nothing but this
//! origin, no framing. Scalar only needs inline styles (it injects `<style>` elements) and `data:`
//! images and fonts. The app's policy would also allow scripts through `'unsafe-inline'`, which the
//! reference does not need.
//!
//! The configuration switches off everything of Scalar that talks to its cloud or hands out tools:
//! no "Ask AI" agent, no MCP, no developer tools (configure, share, deploy, SDK), no client
//! button or "Test request" (no request leaves the page, so no API key is typed into it), no
//! telemetry, no external fonts and no proxy.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

pub const DOCS_PATH: &str = "/api/v1/docs";
pub const SCRIPT_PATH: &str = "/api/v1/docs/scalar.js";
pub const OPENAPI_PATH: &str = "/api/v1/openapi.json";

/// Policy of the reference page: this origin only, scripts from this origin without inline code.
pub const CSP: &str = "default-src 'none'; \
    script-src 'self'; \
    style-src 'self' 'unsafe-inline'; \
    img-src 'self' data:; \
    font-src 'self' data:; \
    connect-src 'self'; \
    object-src 'none'; \
    base-uri 'none'; \
    form-action 'none'; \
    frame-ancestors 'none'";

static SCALAR_JS: &[u8] = include_bytes!("../../../vendor/scalar/standalone.js");

const PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Tijdsregistratie API</title>
</head>
<body>
<script id="api-reference" data-url="/api/v1/openapi.json" data-configuration='{"withDefaultFonts":false,"telemetry":false,"showDeveloperTools":"never","hideClientButton":true,"hideTestRequestButton":true,"persistAuth":false,"agent":{"disabled":true},"mcp":{"disabled":true}}'></script>
<script src="/api/v1/docs/scalar.js"></script>
</body>
</html>
"#;

pub async fn page() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CSP),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        PAGE,
    )
        .into_response()
}

pub async fn script() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        SCALAR_JS,
    )
        .into_response()
}
