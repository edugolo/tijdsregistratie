//! Shared application state, available to server functions via a request extension.

use std::sync::Arc;

use crypto::{BlindIndexKey, DataKeys};
use sqlx::PgPool;
use webauthn_rs::{Webauthn, WebauthnBuilder, prelude::Url};

use super::config::AppConfig;
use super::geocode::Geocoder;

const RP_NAME: &str = "Tijdsregistratie";

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub webauthn: Arc<Webauthn>,
    pub config: Arc<AppConfig>,
    pub keys: Arc<DataKeys>,
    pub blind: Arc<BlindIndexKey>,
    pub geocoder: Arc<Geocoder>,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        config: AppConfig,
        keys: DataKeys,
        blind: BlindIndexKey,
    ) -> anyhow::Result<Self> {
        let origin = Url::parse(&config.app_origin)?;
        let geocoder = Geocoder::new(config.geocoder_url.clone())?;
        let webauthn = WebauthnBuilder::new(&config.rp_id, &origin)?
            .rp_name(RP_NAME)
            .build()?;
        Ok(Self {
            pool,
            webauthn: Arc::new(webauthn),
            config: Arc::new(config),
            keys: Arc::new(keys),
            blind: Arc::new(blind),
            geocoder: Arc::new(geocoder),
        })
    }
}
