//! Server-only code: auth, sessions. The repository lives in the `store` crate. Server function declarations live in `crate::api`.

pub mod auth;
pub mod clock;
pub mod config;
pub mod passkey;
pub mod session;
pub mod staff;
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
    let keys = crypto::DataKeys::from_file(&config.data_key_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot load data key from DATA_KEY_FILE ({}): {e}",
            config.data_key_file.display()
        )
    })?;
    let blind = crypto::BlindIndexKey::from_file(&config.blind_index_key_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot load blind index key from BLIND_INDEX_KEY_FILE ({}): {e}",
            config.blind_index_key_file.display()
        )
    })?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    let session_layer = session::layer(pool.clone(), config.cookie_secure).await?;
    let state = AppState::new(pool, config, keys, blind)?;
    Ok(dioxus::server::router(app)
        .layer(Extension(state))
        .layer(session_layer))
}
