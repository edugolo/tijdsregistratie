//! Server function declarations. Compiled for both targets: the macro keeps the body on the
//! server and generates an HTTP call on the client. Bodies delegate to `crate::server`.

pub mod auth;
pub mod clock;
pub mod geocode;
pub mod setup;
pub mod staff;

use dioxus::prelude::*;

/// Liveness probe: checks that the database answers.
#[get("/api/health")]
pub async fn health() -> Result<String> {
    let state = crate::server::app_state().await?;
    crate::server::check_database(&state.pool).await?;
    Ok(crate::server::health_status().to_string())
}
