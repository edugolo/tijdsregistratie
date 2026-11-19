//! Browser capabilities behind one interface. The server build (SSR) has no browser: it gets
//! inert stand-ins, which are never reached because effects and event handlers only run in the
//! browser.

use crate::api::clock::FixDto;

/// Why a passkey ceremony did not complete.
// Only the browser build can cancel a ceremony; the server build never reaches it.
#[cfg_attr(not(feature = "web"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyError {
    /// The person cancelled or the prompt timed out.
    Cancelled,
    /// This device cannot do passkeys.
    Unsupported,
    Failed,
}

#[cfg(feature = "web")]
mod imp {
    use webauthn_rs_proto::{
        CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
        RequestChallengeResponse,
    };

    use super::{FixDto, PasskeyError};
    use crate::client::glue::{geolocation, platform, webauthn};

    pub fn init_install_prompt() {
        platform::init_install_prompt();
    }

    pub fn is_standalone() -> bool {
        platform::is_standalone()
    }

    pub fn is_ios_safari() -> bool {
        platform::is_ios_safari()
    }

    pub fn can_prompt_install() -> bool {
        platform::can_prompt_install()
    }

    /// Shows the native install dialog; the outcome does not change the flow.
    pub async fn prompt_install() {
        let _ = platform::prompt_install().await;
    }

    pub fn vibrate(ms: u32) {
        platform::vibrate(ms);
    }

    pub async fn sleep_ms(ms: u32) {
        platform::sleep_ms(i32::try_from(ms).unwrap_or(i32::MAX)).await;
    }

    /// One reading; `None` when denied, unavailable or timed out.
    pub async fn current_fix(timeout_ms: u32) -> Option<FixDto> {
        let fix = geolocation::current_position(timeout_ms).await.ok()?;
        Some(FixDto {
            lat: fix.lat,
            lng: fix.lng,
            accuracy_m: fix.accuracy_m,
        })
    }

    pub async fn location_granted() -> bool {
        geolocation::permission_granted().await
    }

    fn map_error(error: webauthn::WebauthnClientError) -> PasskeyError {
        match error {
            webauthn::WebauthnClientError::NotAllowed => PasskeyError::Cancelled,
            webauthn::WebauthnClientError::NotSupported => PasskeyError::Unsupported,
            _ => PasskeyError::Failed,
        }
    }

    pub async fn create_passkey(
        options: CreationChallengeResponse,
    ) -> Result<RegisterPublicKeyCredential, PasskeyError> {
        webauthn::create_passkey(options).await.map_err(map_error)
    }

    pub async fn get_passkey(
        options: RequestChallengeResponse,
    ) -> Result<PublicKeyCredential, PasskeyError> {
        webauthn::get_passkey(options).await.map_err(map_error)
    }
}

#[cfg(not(feature = "web"))]
mod imp {
    use webauthn_rs_proto::{
        CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
        RequestChallengeResponse,
    };

    use super::{FixDto, PasskeyError};

    pub fn init_install_prompt() {}

    pub fn is_standalone() -> bool {
        false
    }

    pub fn is_ios_safari() -> bool {
        false
    }

    pub fn can_prompt_install() -> bool {
        false
    }

    pub async fn prompt_install() {}

    pub fn vibrate(_ms: u32) {}

    pub async fn sleep_ms(_ms: u32) {
        std::future::pending::<()>().await;
    }

    pub async fn current_fix(_timeout_ms: u32) -> Option<FixDto> {
        None
    }

    pub async fn location_granted() -> bool {
        false
    }

    pub async fn create_passkey(
        _options: CreationChallengeResponse,
    ) -> Result<RegisterPublicKeyCredential, PasskeyError> {
        Err(PasskeyError::Unsupported)
    }

    pub async fn get_passkey(
        _options: RequestChallengeResponse,
    ) -> Result<PublicKeyCredential, PasskeyError> {
        Err(PasskeyError::Unsupported)
    }
}

pub use imp::*;
