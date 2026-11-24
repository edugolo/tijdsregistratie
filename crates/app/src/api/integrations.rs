//! Server functions for the Integraties page: management of API keys for the public REST API.
//! Admin only. Timestamps are unix milliseconds. The logic lives in `crate::server::api_v1::keys`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The DTOs are only constructed on the server until the Integraties page (next task) uses them
// in the browser, so the web build sees them as unused.
/// An API key as listed: never contains the secret, only a display prefix.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub struct ApiKeyDto {
    pub id: Uuid,
    pub name: String,
    /// Display prefix like `tr_live_Ab3dEf9h`.
    pub prefix: String,
    /// Scope names: `people:read`, `people:write`, `hours:read`, `export:read`.
    pub scopes: Vec<String>,
    /// `None`: all sites.
    pub site_ids: Option<Vec<Uuid>>,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub last_used_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
}

/// The answer to creating a key: `key` is the only time the secret is available.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub struct CreatedKeyDto {
    pub key: String,
    pub info: ApiKeyDto,
}

/// All keys, newest first, revoked ones included.
#[post("/api/integrations/keys")]
pub async fn api_keys() -> Result<Vec<ApiKeyDto>, ServerFnError> {
    use crate::server::{api_v1::keys, app_state, auth};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(keys::list(&state.pool, &scope).await?)
}

/// `site_ids` `None` gives access to all sites; `expires_in_days` `None` never expires.
#[post("/api/integrations/keys/create")]
pub async fn create_api_key(
    name: String,
    scopes: Vec<String>,
    site_ids: Option<Vec<Uuid>>,
    expires_in_days: Option<u32>,
) -> Result<CreatedKeyDto, ServerFnError> {
    use crate::server::{api_v1::keys, app_state, auth};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    let request = keys::CreateKey {
        name,
        scopes,
        site_ids,
        expires_in_days,
    };
    Ok(keys::create(
        &state.pool,
        &state.keys,
        &scope,
        request,
        chrono::Utc::now(),
    )
    .await?)
}

#[post("/api/integrations/keys/revoke")]
pub async fn revoke_api_key(id: Uuid) -> Result<(), ServerFnError> {
    use crate::server::{api_v1::keys, app_state, auth};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(keys::revoke(&state.pool, &scope, id).await?)
}
