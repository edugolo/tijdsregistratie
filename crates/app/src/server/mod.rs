//! Server-only code: auth, sessions. The repository lives in the `store` crate. Server function declarations live in `crate::api`.

pub mod auth;
pub mod clock;
pub mod config;
pub mod passkey;
pub mod session;
pub mod setup;
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

/// Liveness check for the health endpoint. A failure is logged with its details and reported
/// to the caller as a bare 503, so database errors (host names, SQL) never leave the server.
pub async fn check_database(pool: &sqlx::PgPool) -> Result<(), ServerFnError> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .map(drop)
        .map_err(|e| {
            tracing::error!(error = %e, "health check: database query failed");
            ServerFnError::ServerError {
                message: "unavailable".to_string(),
                code: 503,
                details: None,
            }
        })
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

/// Builds the axum router: Dioxus app and server functions, with state, session and HTTP guard layers.
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
    let session_layer = session::layer(pool.clone(), &config).await?;
    let guard = auth::http::HttpGuard::new(&config);
    let state = AppState::new(pool, config, keys, blind)?;
    let router = dioxus::server::router(app)
        .layer(Extension(state))
        .layer(axum::middleware::from_fn(
            auth::http::refresh_session_expiry,
        ))
        .layer(session_layer);
    Ok(auth::http::protect(router, guard))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn health_failure_is_generic() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(3))
            .connect_lazy("postgres://user:hunter2@db.invalid:5432/tijd")
            .unwrap();
        let err = check_database(&pool).await.unwrap_err();
        let ServerFnError::ServerError {
            message,
            code,
            details,
        } = err
        else {
            panic!("expected a server error");
        };
        assert_eq!((message.as_str(), code), ("unavailable", 503));
        assert!(details.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn health_ok(pool: sqlx::PgPool) {
        check_database(&pool).await.unwrap();
        assert_eq!(health_status(), "ok");
    }
}
