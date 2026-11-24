//! Browser features the wizard needs, behind plain types. Without the `web` feature (server
//! build) they report "not available"; they are only ever awaited in the browser.

use webauthn_rs_proto::{CreationChallengeResponse, RegisterPublicKeyCredential};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub enum LocateFailure {
    Denied,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub enum PasskeyFailure {
    /// Cancelled or timed out.
    Cancelled,
    Unsupported,
    Failed,
}

/// Reads the current position once and returns `(lat, lng)`.
#[cfg(feature = "web")]
pub async fn locate() -> Result<(f64, f64), LocateFailure> {
    use crate::client::glue::geolocation::{GeoError, current_position};
    match current_position(20_000).await {
        Ok(fix) => Ok((fix.lat, fix.lng)),
        Err(GeoError::Denied) => Err(LocateFailure::Denied),
        Err(_) => Err(LocateFailure::Unavailable),
    }
}

#[cfg(not(feature = "web"))]
pub async fn locate() -> Result<(f64, f64), LocateFailure> {
    Err(LocateFailure::Unavailable)
}

/// Asks the device to create a passkey.
#[cfg(feature = "web")]
pub async fn create_passkey(
    options: CreationChallengeResponse,
) -> Result<RegisterPublicKeyCredential, PasskeyFailure> {
    use crate::client::glue::webauthn::{WebauthnClientError, create_passkey};
    create_passkey(options).await.map_err(|e| match e {
        WebauthnClientError::NotAllowed | WebauthnClientError::InvalidState => {
            PasskeyFailure::Cancelled
        }
        WebauthnClientError::NotSupported => PasskeyFailure::Unsupported,
        WebauthnClientError::Other(_) => PasskeyFailure::Failed,
    })
}

#[cfg(not(feature = "web"))]
pub async fn create_passkey(
    _options: CreationChallengeResponse,
) -> Result<RegisterPublicKeyCredential, PasskeyFailure> {
    Err(PasskeyFailure::Unsupported)
}

/// Makes the browser fetch `/theme.css` again, so saved brand colours apply without a reload.
/// The query string changes the URL; the server ignores it.
#[cfg(feature = "web")]
pub fn reload_theme() {
    let link = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.query_selector("link[href^=\"/theme.css\"]").ok())
        .flatten();
    if let Some(link) = link {
        let stamp = js_sys::Date::now();
        // Setting an attribute only fails for invalid names; this one is fixed.
        let _ = link.set_attribute("href", &format!("/theme.css?v={stamp}"));
    }
}

#[cfg(not(feature = "web"))]
pub fn reload_theme() {}
