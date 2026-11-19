//! Server function declarations. Compiled for both targets: the macro keeps the body on the
//! server and generates an HTTP call on the client. Bodies delegate to `crate::server`.

use dioxus::prelude::*;

/// Liveness probe used by the hello-world page.
#[get("/api/health")]
pub async fn health() -> Result<String> {
    Ok(crate::server::health_status().to_string())
}
