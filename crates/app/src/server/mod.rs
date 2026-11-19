//! Server-only code: auth, sessions, repository. Server function declarations live in `crate::api`.

pub mod config;
pub mod session;
pub mod state;

use axum::Extension;
use dioxus::fullstack::FullstackContext;
use dioxus::prelude::ServerFnError;
use tower_sessions::Session;

pub use state::AppState;

/// Status reported by the health server function.
pub fn health_status() -> &'static str {
    "ok"
}

/// Fetches the `AppState` from the current server function request.
pub async fn app_state() -> Result<AppState, ServerFnError> {
    let Extension(state) = FullstackContext::extract::<Extension<AppState>, _>().await?;
    Ok(state)
}

/// Fetches the request's `Session` from the current server function request.
pub async fn session() -> Result<Session, ServerFnError> {
    FullstackContext::extract::<Session, _>().await
}

/// Builds the axum router: Dioxus app and server functions, with state and session layers.
pub async fn build_router(app: fn() -> dioxus::prelude::Element) -> anyhow::Result<axum::Router> {
    let config = config::AppConfig::from_env()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    let session_layer = session::layer(pool.clone(), config.cookie_secure).await?;
    let state = AppState::new(pool, config)?;
    Ok(dioxus::server::router(app)
        .layer(Extension(state))
        .layer(session_layer))
}
