//! Server functions for sign-in, enrollment and sessions.
//!
//! The ceremonies live in `crate::server::passkey`. Errors reaching the client are generic;
//! details are logged on the server (ids only).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs_proto::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

/// What the enrollment page may know about a token before registering a passkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentInfo {
    pub valid: bool,
    /// First word of the person's name, only present when the token is valid.
    pub first_name: Option<String>,
}

/// The signed-in person, as far as the client needs to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    pub person_id: Uuid,
    pub first_name: String,
    /// Manager of a site, or admin.
    pub is_staff: bool,
    pub is_admin: bool,
    pub employee_site_ids: Vec<Uuid>,
}

/// Whether an enrollment token can still be used. Never reveals more than that.
#[post("/api/auth/enrollment-info")]
pub async fn enrollment_info(token: String) -> Result<EnrollmentInfo> {
    let state = crate::server::app_state().await?;
    Ok(crate::server::passkey::enrollment_info(&state, &token).await)
}

/// Starts passkey registration for the person the token belongs to.
#[post("/api/auth/register/begin")]
pub async fn begin_registration(token: String) -> Result<CreationChallengeResponse> {
    let state = crate::server::app_state().await?;
    let session = crate::server::session().await?;
    crate::server::passkey::begin_registration(&state, &session, &token)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// Verifies the new passkey, consumes the token and signs the person in.
#[post("/api/auth/register/finish")]
pub async fn finish_registration(credential: RegisterPublicKeyCredential) -> Result<Me> {
    let state = crate::server::app_state().await?;
    let session = crate::server::session().await?;
    crate::server::passkey::finish_registration(&state, &session, &credential)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// Starts a discoverable (usernameless) passkey login.
#[post("/api/auth/login/begin")]
pub async fn begin_login() -> Result<RequestChallengeResponse> {
    let state = crate::server::app_state().await?;
    let session = crate::server::session().await?;
    crate::server::passkey::begin_login(&state, &session)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

#[post("/api/auth/login/finish")]
pub async fn finish_login(credential: PublicKeyCredential) -> Result<Me> {
    let state = crate::server::app_state().await?;
    let session = crate::server::session().await?;
    crate::server::passkey::finish_login(&state, &session, &credential)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

#[post("/api/auth/logout")]
pub async fn logout() -> Result<()> {
    let session = crate::server::session().await?;
    crate::server::passkey::logout(&session)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// The signed-in person, or `None` (no error) when there is no valid session.
#[get("/api/auth/me")]
pub async fn me() -> Result<Option<Me>> {
    let state = crate::server::app_state().await?;
    let session = crate::server::session().await?;
    crate::server::passkey::me(&state, &session)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}
