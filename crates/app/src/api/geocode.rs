//! Address search (OpenStreetMap open data) for the school editor and the setup wizard.
//!
//! The logic lives in `crate::server::geocode`. The browser never talks to the geocoder itself:
//! the server does, with a global rate limit and a cache, as the Nominatim usage policy asks.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Shortest and longest accepted search text, in characters (after trimming).
pub const MIN_QUERY_CHARS: usize = 3;
pub const MAX_QUERY_CHARS: usize = 200;

/// One address match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoSuggestion {
    pub label: String,
    pub lat: f64,
    pub lng: f64,
}

/// Whether the address search is switched on (`GEOCODER_URL`). Public: it only reveals a flag.
#[get("/api/geocode/enabled")]
pub async fn geocoder_enabled() -> Result<bool> {
    let state = crate::server::app_state().await?;
    Ok(state.geocoder.enabled())
}

/// Searches an address. Admins only; during first-run setup also a session that already entered
/// a valid setup code.
#[post("/api/geocode")]
pub async fn geocode(query: String) -> Result<Vec<GeoSuggestion>> {
    let state = crate::server::app_state().await?;
    let session = crate::server::session().await?;
    crate::server::auth::require_admin_or_setup(&state.pool, &session).await?;
    state
        .geocoder
        .search(&query)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}
