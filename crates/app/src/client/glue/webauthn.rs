//! Passkey (WebAuthn) ceremonies via `navigator.credentials`, using `webauthn-rs-proto` types.
//!
//! Credential contents are never logged here. Note: the proto crate's own
//! `From<web_sys::PublicKeyCredential> for PublicKeyCredential` logs the client extension outputs
//! (not the credential id, keys or signature) with `console.log`.

use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use webauthn_rs_proto::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebauthnClientError {
    /// The user cancelled, or the prompt timed out (`NotAllowedError`).
    #[error("passkey request not allowed (cancelled or timed out)")]
    NotAllowed,
    /// This authenticator already holds a passkey for the account (`InvalidStateError`).
    #[error("passkey already registered")]
    InvalidState,
    /// Passkeys are not supported here (`NotSupportedError` or no WebAuthn API).
    #[error("passkeys not supported")]
    NotSupported,
    #[error("passkey error: {0}")]
    Other(String),
}

/// Maps a `DOMException.name` (plus message for the fallback) to a [`WebauthnClientError`].
pub fn map_dom_exception(name: &str, message: &str) -> WebauthnClientError {
    match name {
        "NotAllowedError" | "AbortError" => WebauthnClientError::NotAllowed,
        "InvalidStateError" => WebauthnClientError::InvalidState,
        "NotSupportedError" => WebauthnClientError::NotSupported,
        _ => WebauthnClientError::Other(format!("{name}: {message}")),
    }
}

fn map_js_error(err: &JsValue) -> WebauthnClientError {
    let field = |key: &str| {
        Reflect::get(err, &key.into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default()
    };
    map_dom_exception(&field("name"), &field("message"))
}

fn credentials() -> Result<web_sys::CredentialsContainer, WebauthnClientError> {
    if !passkeys_supported() {
        return Err(WebauthnClientError::NotSupported);
    }
    web_sys::window()
        .map(|w| w.navigator().credentials())
        .ok_or(WebauthnClientError::NotSupported)
}

async fn resolve(
    promise: Result<js_sys::Promise, JsValue>,
) -> Result<web_sys::PublicKeyCredential, WebauthnClientError> {
    let value = JsFuture::from(promise.map_err(|e| map_js_error(&e))?)
        .await
        .map_err(|e| map_js_error(&e))?;
    value
        .dyn_into::<web_sys::PublicKeyCredential>()
        .map_err(|_| WebauthnClientError::Other("unexpected credential type".into()))
}

/// `navigator.credentials.create({ publicKey })`.
pub async fn create_passkey(
    options: CreationChallengeResponse,
) -> Result<RegisterPublicKeyCredential, WebauthnClientError> {
    let creds = credentials()?;
    let opts = web_sys::CredentialCreationOptions::from(options);
    let cred = resolve(creds.create_with_options(&opts)).await?;
    Ok(RegisterPublicKeyCredential::from(cred))
}

/// `navigator.credentials.get({ publicKey })`. Works for discoverable credentials when the
/// server sends an empty `allowCredentials`.
pub async fn get_passkey(
    options: RequestChallengeResponse,
) -> Result<PublicKeyCredential, WebauthnClientError> {
    let creds = credentials()?;
    let opts = web_sys::CredentialRequestOptions::from(options);
    let cred = resolve(creds.get_with_options(&opts)).await?;
    Ok(PublicKeyCredential::from(cred))
}

/// True when the browser exposes `window.PublicKeyCredential`.
pub fn passkeys_supported() -> bool {
    web_sys::window()
        .and_then(|w| Reflect::has(&w, &"PublicKeyCredential".into()).ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dom_exceptions() {
        assert_eq!(
            map_dom_exception("NotAllowedError", "x"),
            WebauthnClientError::NotAllowed
        );
        assert_eq!(
            map_dom_exception("AbortError", "x"),
            WebauthnClientError::NotAllowed
        );
        assert_eq!(
            map_dom_exception("InvalidStateError", "x"),
            WebauthnClientError::InvalidState
        );
        assert_eq!(
            map_dom_exception("NotSupportedError", "x"),
            WebauthnClientError::NotSupported
        );
        assert_eq!(
            map_dom_exception("SecurityError", "bad rp"),
            WebauthnClientError::Other("SecurityError: bad rp".into())
        );
    }
}
