//! The interactive API reference (Scalar) and the OpenAPI document.
//!
//! The Scalar standalone script is vendored (`public/vendor/scalar/`, see its README) and
//! embedded in the binary, so it is served from this origin and the app's strict CSP
//! (`script-src 'self'`, no CDN) holds without a relaxed policy for this page: the page has no
//! inline script, its configuration sits in `data-` attributes.

use axum::http::{HeaderValue, header};
use axum::response::{Html, IntoResponse, Response};

pub const DOCS_PATH: &str = "/api/v1/docs";
pub const SCRIPT_PATH: &str = "/api/v1/docs/scalar.js";
pub const OPENAPI_PATH: &str = "/api/v1/openapi.json";

static SCALAR_JS: &[u8] = include_bytes!("../../../../../public/vendor/scalar/standalone.js");

const PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Tijdsregistratie API</title>
</head>
<body>
<script id="api-reference" data-url="/api/v1/openapi.json" data-configuration='{"withDefaultFonts":false,"telemetry":false}'></script>
<script src="/api/v1/docs/scalar.js"></script>
</body>
</html>
"#;

pub async fn page() -> Html<&'static str> {
    Html(PAGE)
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
