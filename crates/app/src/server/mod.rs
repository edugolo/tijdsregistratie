//! Server-only code: auth, sessions. The repository lives in the `store` crate. Server function declarations live in `crate::api`.

pub mod auth;
pub mod clock;
pub mod config;
pub mod passkey;
pub mod session;
pub mod setup;
pub mod staff;
pub mod state;
pub mod theme;

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

/// Runs the app: binds `IP`/`PORT` (default 127.0.0.1:8080) and serves the router with the peer
/// address attached to every request (`dioxus::serve` does not, which per-client rate limiting
/// needs). Does not return.
pub fn serve(app: fn() -> dioxus::prelude::Element) -> ! {
    dioxus::logger::initialize_default();
    let runtime = tokio::runtime::Runtime::new().expect("cannot start the async runtime");
    let result: anyhow::Result<()> = runtime.block_on(async {
        let addr = dioxus::cli_config::fullstack_address_or_localhost();
        let router = build_router(app).await?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!("listening on {addr}");
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
        Ok(())
    });
    match result {
        Ok(()) => unreachable!("the server never stops on its own"),
        Err(e) => {
            eprintln!("fatal: {e:#}");
            std::process::exit(1);
        }
    }
}

/// One warning per key file that group or others can access.
fn key_permission_warnings(config: &config::AppConfig) -> Vec<String> {
    #[cfg(unix)]
    {
        [
            ("DATA_KEY_FILE", &config.data_key_file),
            ("BLIND_INDEX_KEY_FILE", &config.blind_index_key_file),
        ]
        .into_iter()
        .filter_map(|(name, path)| {
            crypto::check_permissions(path)
                .err()
                .map(|w| format!("{name}: {w}"))
        })
        .collect()
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        Vec::new()
    }
}

/// Builds the axum router: Dioxus app and server functions, with state, session and HTTP guard layers.
pub async fn build_router(app: fn() -> dioxus::prelude::Element) -> anyhow::Result<axum::Router> {
    let config = config::AppConfig::from_env()?;
    for w in key_permission_warnings(&config) {
        tracing::warn!("{w}");
    }
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
    // In setup mode this puts the setup code in the log right after boot (the lazy path in
    // `setup::required` still covers expiry later on). After completion it does nothing.
    setup::required(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("setup check failed: {e:?}"))?;
    let session_layer = session::layer(pool.clone(), &config).await?;
    let guard = auth::http::HttpGuard::new(&config);
    let state = AppState::new(pool, config, keys, blind)?;
    let router = dioxus::server::router(app)
        .route("/theme.css", axum::routing::get(theme::theme_css))
        .layer(Extension(state));
    let router = auth::http::with_session(router, session_layer, guard.clone());
    Ok(auth::http::protect(router, guard))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn group_or_world_readable_key_files_are_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("keys-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let (data, blind) = (dir.join("data"), dir.join("blind"));
        for (p, mode) in [(&data, 0o600), (&blind, 0o640)] {
            std::fs::write(p, "x").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let env = [
            ("DATABASE_URL", "postgres://unused"),
            ("APP_ORIGIN", "https://tijd.example.org"),
            ("DATA_KEY_FILE", data.to_str().unwrap()),
            ("BLIND_INDEX_KEY_FILE", blind.to_str().unwrap()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let config = config::AppConfig::from_map(&env).unwrap();
        let w = key_permission_warnings(&config);
        assert_eq!(w.len(), 1);
        assert!(w[0].starts_with("BLIND_INDEX_KEY_FILE") && w[0].contains("640"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
