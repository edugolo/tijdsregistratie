//! Shared application state, available to server functions via a request extension.

use std::sync::Arc;

use sqlx::PgPool;
use webauthn_rs::{Webauthn, WebauthnBuilder, prelude::Url};

use super::config::AppConfig;

const RP_NAME: &str = "Tijdsregistratie";

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub webauthn: Arc<Webauthn>,
    pub config: Arc<AppConfig>,
    // TODO(T5): crypto keys
}

impl AppState {
    pub fn new(pool: PgPool, config: AppConfig) -> anyhow::Result<Self> {
        let origin = Url::parse(&config.app_origin)?;
        let webauthn = WebauthnBuilder::new(&config.rp_id, &origin)?
            .rp_name(RP_NAME)
            .build()?;
        Ok(Self {
            pool,
            webauthn: Arc::new(webauthn),
            config: Arc::new(config),
        })
    }
}
