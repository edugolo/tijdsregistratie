//! Passkey ceremonies: WebAuthn registration and authentication state handling.
//!
//! # Login approach: discoverable passkeys
//!
//! Login needs no username. `begin_login` uses webauthn-rs discoverable authentication (cargo
//! feature `conditional-ui` of `webauthn-rs` 0.6.2, which gates `start_discoverable_authentication`;
//! the feature name is historical, the API is the generic discoverable flow). The library forces
//! `mediation: conditional` (autofill UI); we reset it to the default (modal) because the login
//! screen is a button, not a username field. The authenticator returns the user handle with the
//! assertion; `identify_discoverable_authentication` extracts it together with the credential id,
//! and we load exactly that credential from the database. Registration asks for a resident key
//! (`residentKey: preferred`, patched into the options because `start_passkey_registration`
//! defaults to discouraged) so that platform authenticators create discoverable credentials.
//! No "remembered person" cookie is needed.
//!
//! The WebAuthn user handle is the 16 bytes of a random v4 UUID stored on the person, not the
//! person id and not derived from the name.
//!
//! Client-facing errors are generic. Internals are logged with ids only, never tokens or
//! credential material.

use chrono::{DateTime, Utc};
use dioxus::prelude::ServerFnError;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableAuthentication, Passkey, PasskeyAuthentication,
    PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};
use webauthn_rs_proto::ResidentKeyRequirement;

use super::AppState;
use super::session::{self, SessionKind, SessionUser};
use crate::api::auth::{EnrollmentInfo, Me};
use store::repo::person::{self, Person};
use store::repo::role::{self, scope_from_roles};
use store::repo::{audit, enrollment, passkey};

const REGISTRATION_KEY: &str = "ceremony.registration";
const LOGIN_KEY: &str = "ceremony.login";
const REAUTH_KEY: &str = "ceremony.reauth";

/// Stable codes; the client translates them through `i18n/nl-BE/app.ftl`.
const MSG_TOKEN: &str = "token-invalid";
const MSG_LOGIN: &str = "login-failed";

/// Failure of a ceremony step. The client only ever sees the generic message of the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// Token unknown, used, expired or not matching the ceremony.
    Token,
    /// Verification failed, passkey revoked, person inactive, no ceremony in progress.
    Login,
    /// Database, crypto or session failure.
    Internal,
}

impl From<AuthError> for ServerFnError {
    fn from(e: AuthError) -> Self {
        let (code, message) = match e {
            AuthError::Token => (400, MSG_TOKEN),
            AuthError::Login => (401, MSG_LOGIN),
            AuthError::Internal => (500, MSG_LOGIN),
        };
        ServerFnError::ServerError {
            message: message.to_string(),
            code,
            details: None,
        }
    }
}

type Result<T> = std::result::Result<T, AuthError>;

fn internal(what: &'static str) -> impl FnOnce(&dyn std::fmt::Display) -> AuthError {
    move |e| {
        tracing::error!(error = %e, "{what}");
        AuthError::Internal
    }
}

fn db(e: store::repo::RepoError) -> AuthError {
    internal("repository call failed")(&e)
}

fn sess(e: tower_sessions::session::Error) -> AuthError {
    internal("session call failed")(&e)
}

#[derive(Serialize, Deserialize)]
struct RegistrationCeremony {
    state: PasskeyRegistration,
    person_id: Uuid,
    /// SHA-256 of the enrollment token; the raw token never enters the session.
    token_hash: Vec<u8>,
    /// Whether the token was found through its typed code rather than the link (audit only).
    /// Absent in ceremonies started before this field existed: those came through the link.
    #[serde(default)]
    via_code: bool,
}

fn first_name(display_name: &str) -> String {
    display_name
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

async fn active_person<'e>(
    exec: impl sqlx::PgExecutor<'e>,
    keys: &crypto::DataKeys,
    id: Uuid,
) -> Result<Person> {
    match person::get_person(exec, keys, id).await.map_err(db)? {
        Some(p) if p.active => Ok(p),
        _ => Err(AuthError::Login),
    }
}

/// Builds the client view and the session kind from the person's roles.
async fn me_of(state: &AppState, p: &Person) -> Result<(Me, SessionKind)> {
    let roles = role::roles_of(&state.pool, p.id).await.map_err(db)?;
    let scope = scope_from_roles(p.id, &roles);
    let employee_site_ids = role::employee_sites(&state.pool, p.id).await.map_err(db)?;
    let is_staff = scope.is_some();
    let me = Me {
        person_id: p.id,
        first_name: first_name(&p.display_name),
        is_staff,
        is_admin: scope.is_some_and(|s| s.is_admin()),
        employee_site_ids,
    };
    let kind = if is_staff {
        SessionKind::Staff
    } else {
        SessionKind::Employee
    };
    Ok((me, kind))
}

async fn sign_in(state: &AppState, session: &Session, p: &Person, passkey_id: Uuid) -> Result<Me> {
    let (me, kind) = me_of(state, p).await?;
    session::set_user(
        session,
        SessionUser {
            person_id: p.id,
            kind,
            authenticated_at: chrono::Utc::now(),
            passkey_id,
        },
    )
    .await
    .map_err(sess)?;
    Ok(me)
}

fn user_uuid(p: &Person) -> Result<Uuid> {
    Uuid::from_slice(&p.user_handle).map_err(|_| internal("invalid user handle")(&"length"))
}

/// Valid token and the first name of its person; `valid: false` and no name otherwise.
pub async fn enrollment_info(state: &AppState, token: &str) -> EnrollmentInfo {
    let invalid = EnrollmentInfo {
        valid: false,
        first_name: None,
    };
    let Ok(Some(person_id)) = enrollment::peek_valid(&state.pool, token).await else {
        return invalid;
    };
    match active_person(&state.pool, &state.keys, person_id).await {
        Ok(p) => EnrollmentInfo {
            valid: true,
            first_name: Some(first_name(&p.display_name)),
        },
        Err(_) => invalid,
    }
}

/// Like [`enrollment_info`], for a typed code. An unknown, malformed, used or expired code, or
/// one of an inactive person, is one generic [`AuthError::Token`]: a failure here is what the
/// HTTP guard counts against the guessing limits.
pub async fn enrollment_info_by_code(state: &AppState, code: &str) -> Result<EnrollmentInfo> {
    let found = code_match(state, code).await?;
    let p = active_person(&state.pool, &state.keys, found.person_id)
        .await
        .map_err(|_| AuthError::Token)?;
    Ok(EnrollmentInfo {
        valid: true,
        first_name: Some(first_name(&p.display_name)),
    })
}

/// The valid token behind a typed code, or the generic token error.
async fn code_match(state: &AppState, code: &str) -> Result<enrollment::CodeMatch> {
    enrollment::peek_code(&state.pool, &state.blind, code)
        .await
        .map_err(db)?
        .ok_or(AuthError::Token)
}

pub async fn begin_registration(
    state: &AppState,
    session: &Session,
    token: &str,
) -> Result<CreationChallengeResponse> {
    let person_id = enrollment::peek_valid(&state.pool, token)
        .await
        .map_err(db)?
        .ok_or(AuthError::Token)?;
    start_registration(
        state,
        session,
        person_id,
        enrollment::token_hash(token),
        false,
    )
    .await
}

/// Starts the same registration ceremony as [`begin_registration`] for the token a typed code
/// belongs to. The ceremony carries that token's hash, so [`finish_registration`] consumes the
/// token (and with it the code); the raw token is never looked at or sent anywhere.
pub async fn begin_registration_with_code(
    state: &AppState,
    session: &Session,
    code: &str,
) -> Result<CreationChallengeResponse> {
    let found = code_match(state, code).await?;
    start_registration(state, session, found.person_id, found.token_hash, true).await
}

/// The ceremony for the token known by `token_hash`; the session keeps only that hash.
async fn start_registration(
    state: &AppState,
    session: &Session,
    person_id: Uuid,
    token_hash: Vec<u8>,
    via_code: bool,
) -> Result<CreationChallengeResponse> {
    let p = active_person(&state.pool, &state.keys, person_id)
        .await
        .map_err(|_| AuthError::Token)?;
    let exclude: Vec<_> = existing_passkeys(state, person_id)
        .await?
        .iter()
        .map(|k| k.cred_id().clone())
        .collect();
    let (mut challenge, reg) = state
        .webauthn
        .start_passkey_registration(
            user_uuid(&p)?,
            &p.display_name,
            &p.display_name,
            (!exclude.is_empty()).then_some(exclude),
        )
        .map_err(|e| internal("start registration failed")(&e))?;
    if let Some(sel) = challenge.public_key.authenticator_selection.as_mut() {
        sel.resident_key = Some(ResidentKeyRequirement::Preferred);
    }
    session::put_ceremony(
        session,
        REGISTRATION_KEY,
        &RegistrationCeremony {
            state: reg,
            person_id,
            token_hash,
            via_code,
        },
    )
    .await
    .map_err(sess)?;
    Ok(challenge)
}

async fn existing_passkeys(state: &AppState, person_id: Uuid) -> Result<Vec<Passkey>> {
    passkey::active_for_person(&state.pool, person_id)
        .await
        .map_err(db)?
        .into_iter()
        .map(|(_, json)| {
            serde_json::from_value(json).map_err(|e| internal("stored passkey unreadable")(&e))
        })
        .collect()
}

/// Longest device label stored (the database enforces the same limit).
const MAX_LABEL_CHARS: usize = 60;

/// Non-personal description of a device like "iPhone – Safari", from its User-Agent. Only a
/// coarse system and browser name is kept, never the raw header. `None` if neither is known.
pub fn device_label(user_agent: &str) -> Option<String> {
    let has = |needle: &str| user_agent.contains(needle);
    let system = if has("iPhone") {
        Some("iPhone")
    } else if has("iPad") {
        Some("iPad")
    } else if has("Android") {
        Some("Android")
    } else if has("Windows") {
        Some("Windows")
    } else if has("CrOS") {
        Some("ChromeOS")
    } else if has("Macintosh") || has("Mac OS X") {
        Some("Mac")
    } else if has("Linux") || has("X11") {
        Some("Linux")
    } else {
        None
    };
    let browser = if has("Edg/") || has("EdgA/") || has("EdgiOS/") {
        Some("Edge")
    } else if has("OPR/") {
        Some("Opera")
    } else if has("Firefox/") || has("FxiOS/") {
        Some("Firefox")
    } else if has("Chrome/") || has("CriOS/") {
        Some("Chrome")
    } else if has("Safari/") {
        Some("Safari")
    } else {
        None
    };
    let label = match (system, browser) {
        (Some(s), Some(b)) => format!("{s} – {b}"),
        (Some(x), None) | (None, Some(x)) => x.to_string(),
        (None, None) => return None,
    };
    Some(label.chars().take(MAX_LABEL_CHARS).collect())
}

pub async fn finish_registration(
    state: &AppState,
    session: &Session,
    credential: &RegisterPublicKeyCredential,
    user_agent: Option<&str>,
) -> Result<Me> {
    let ceremony: RegistrationCeremony = session::take_ceremony(session, REGISTRATION_KEY)
        .await
        .map_err(sess)?
        .ok_or(AuthError::Token)?;
    let new_key = state
        .webauthn
        .finish_passkey_registration(credential, &ceremony.state)
        .map_err(|e| {
            tracing::warn!(person_id = %ceremony.person_id, error = %e, "passkey registration rejected");
            AuthError::Login
        })?;
    // A credential id belongs to exactly one person; check before touching the token.
    if passkey::find_by_credential_id(&state.pool, new_key.cred_id().as_ref())
        .await
        .map_err(db)?
        .is_some()
    {
        tracing::warn!(person_id = %ceremony.person_id, "credential id already registered");
        return Err(AuthError::Login);
    }
    let p = active_person(&state.pool, &state.keys, ceremony.person_id)
        .await
        .map_err(|_| AuthError::Token)?;
    let json =
        serde_json::to_value(&new_key).map_err(|e| internal("serialize passkey failed")(&e))?;
    // Token, passkey and audit row are written in one transaction: a failure leaves the token
    // unused so the person can retry.
    // Staff may link several devices; employees keep exactly one (the new one replaces the old).
    let (_, kind) = me_of(state, &p).await?;
    let devices = match kind {
        SessionKind::Staff => enrollment::Devices::AddToExisting,
        SessionKind::Employee => enrollment::Devices::ReplaceExisting,
    };
    let label = user_agent.and_then(device_label);
    let passkey_id = enrollment::complete(
        &state.pool,
        &ceremony.token_hash,
        p.id,
        enrollment::NewPasskey {
            credential_id: new_key.cred_id().as_ref(),
            credential_json: json,
            label: label.as_deref(),
        },
        devices,
        if ceremony.via_code {
            enrollment::Redemption::Code
        } else {
            enrollment::Redemption::Link
        },
    )
    .await
    .map_err(db)?
    .ok_or_else(|| {
        tracing::warn!(person_id = %ceremony.person_id, "enrollment token no longer valid at finish");
        AuthError::Token
    })?;
    sign_in(state, session, &p, passkey_id).await
}

pub async fn begin_login(state: &AppState, session: &Session) -> Result<RequestChallengeResponse> {
    let (mut challenge, auth) = state
        .webauthn
        .start_discoverable_authentication()
        .map_err(|e| internal("start login failed")(&e))?;
    // The library forces conditional (autofill) mediation; our login is an explicit button.
    challenge.mediation = None;
    session::put_ceremony(session, LOGIN_KEY, &auth)
        .await
        .map_err(sess)?;
    Ok(challenge)
}

pub async fn finish_login(
    state: &AppState,
    session: &Session,
    credential: &PublicKeyCredential,
) -> Result<Me> {
    let auth: DiscoverableAuthentication = session::take_ceremony(session, LOGIN_KEY)
        .await
        .map_err(sess)?
        .ok_or(AuthError::Login)?;
    let (user_handle, credential_id) = state
        .webauthn
        .identify_discoverable_authentication(credential)
        .map_err(|_| AuthError::Login)?;
    // The passkey row stays locked from here to the commit: concurrent logins with one passkey
    // are serialized, so the signature counter check cannot be raced.
    let mut tx = state.pool.begin().await.map_err(|e| db(e.into()))?;
    let stored = passkey::find_by_credential_id_for_update(&mut tx, credential_id)
        .await
        .map_err(db)?
        .ok_or(AuthError::Login)?;
    if stored.revoked_at.is_some() {
        tracing::warn!(person_id = %stored.person_id, "login with revoked passkey");
        return Err(AuthError::Login);
    }
    let p = active_person(&mut *tx, &state.keys, stored.person_id).await?;
    if user_uuid(&p)? != user_handle {
        tracing::warn!(person_id = %p.id, "user handle does not match credential owner");
        return Err(AuthError::Login);
    }
    let mut key: Passkey = serde_json::from_value(stored.credential)
        .map_err(|e| internal("stored passkey unreadable")(&e))?;
    let result = state
        .webauthn
        .finish_discoverable_authentication_v2(credential, auth, &key)
        .map_err(|e| {
            tracing::warn!(person_id = %p.id, error = %e, "passkey login rejected");
            AuthError::Login
        })?;
    key.update_credential(&result);
    let json = serde_json::to_value(&key).map_err(|e| internal("serialize passkey failed")(&e))?;
    passkey::update_after_login(&mut *tx, stored.id, json)
        .await
        .map_err(db)?;
    // The audit row shares the transaction: no audit row, no login.
    audit::record(&mut *tx, Some(p.id), "login", "person", Some(p.id), &[])
        .await
        .map_err(db)?;
    tx.commit().await.map_err(|e| db(e.into()))?;
    sign_in(state, session, &p, stored.id).await
}

#[derive(Serialize, Deserialize)]
struct ReauthCeremony {
    state: PasskeyAuthentication,
    person_id: Uuid,
}

/// The signed-in user of this session, or a login failure.
async fn signed_in_user(state: &AppState, session: &Session) -> Result<SessionUser> {
    session::current(&state.pool, session)
        .await
        .map_err(|e| internal("session check failed")(&e))?
        .ok_or(AuthError::Login)
}

/// Step-up: starts a passkey check for the person of the current session. Not discoverable:
/// only that person's active passkeys are allowed, and user verification is required.
pub async fn begin_reauth(state: &AppState, session: &Session) -> Result<RequestChallengeResponse> {
    let user = signed_in_user(state, session).await?;
    let keys = existing_passkeys(state, user.person_id).await?;
    if keys.is_empty() {
        return Err(AuthError::Login);
    }
    let (challenge, auth) = state
        .webauthn
        .start_passkey_authentication(&keys)
        .map_err(|e| internal("start reauth failed")(&e))?;
    session::put_ceremony(
        session,
        REAUTH_KEY,
        &ReauthCeremony {
            state: auth,
            person_id: user.person_id,
        },
    )
    .await
    .map_err(sess)?;
    Ok(challenge)
}

/// Step-up: verifies the assertion and, only if it is by an active passkey of the session's own
/// person, remembers `now` as the time of the last check.
pub async fn finish_reauth(
    state: &AppState,
    session: &Session,
    credential: &PublicKeyCredential,
    now: DateTime<Utc>,
) -> Result<()> {
    let user = signed_in_user(state, session).await?;
    let ceremony: ReauthCeremony = session::take_ceremony(session, REAUTH_KEY)
        .await
        .map_err(sess)?
        .ok_or(AuthError::Login)?;
    if ceremony.person_id != user.person_id {
        return Err(AuthError::Login);
    }
    let result = state
        .webauthn
        .finish_passkey_authentication(credential, &ceremony.state)
        .map_err(|e| {
            tracing::warn!(person_id = %user.person_id, error = %e, "reauth rejected");
            AuthError::Login
        })?;
    // Row lock to the commit, like a login: the signature counter check cannot be raced.
    let mut tx = state.pool.begin().await.map_err(|e| db(e.into()))?;
    let stored = passkey::find_by_credential_id_for_update(&mut tx, result.cred_id().as_ref())
        .await
        .map_err(db)?
        .ok_or(AuthError::Login)?;
    if stored.revoked_at.is_some() || stored.person_id != user.person_id {
        tracing::warn!(person_id = %user.person_id, "reauth with revoked or foreign passkey");
        return Err(AuthError::Login);
    }
    let mut key: Passkey = serde_json::from_value(stored.credential)
        .map_err(|e| internal("stored passkey unreadable")(&e))?;
    key.update_credential(&result);
    let json = serde_json::to_value(&key).map_err(|e| internal("serialize passkey failed")(&e))?;
    passkey::update_after_login(&mut *tx, stored.id, json)
        .await
        .map_err(db)?;
    audit::record(
        &mut *tx,
        Some(user.person_id),
        "auth.reauth",
        "person",
        Some(user.person_id),
        &[],
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(|e| db(e.into()))?;
    session::mark_reauth(session, user.person_id, now)
        .await
        .map_err(sess)
}

pub async fn logout(session: &Session) -> Result<()> {
    session::clear(session).await.map_err(sess)
}

/// The active signed-in person, or `None`. A deactivated person, a revoked passkey or ended
/// sessions count as signed out.
pub async fn me(state: &AppState, session: &Session) -> Result<Option<Me>> {
    let Some(user) = session::current(&state.pool, session)
        .await
        .map_err(|e| internal("session check failed")(&e))?
    else {
        return Ok(None);
    };
    match active_person(&state.pool, &state.keys, user.person_id).await {
        Ok(p) => Ok(Some(me_of(state, &p).await?.0)),
        Err(AuthError::Login) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    //! Full ceremonies against a software authenticator (`webauthn-authenticator-rs` SoftPasskey),
    //! calling the internal functions with an in-memory session instead of going over HTTP.

    use std::collections::HashMap;
    use std::sync::Arc;

    use chrono::Duration;
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};
    use sqlx::PgPool;
    use tower_sessions::{MemoryStore, Session};
    use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
    use webauthn_rs::prelude::Url;
    use webauthn_rs_proto::AllowCredentials;

    use super::*;
    use store::repo::role::Role;

    const ORIGIN: &str = "https://tijd.example.org";

    fn state(pool: PgPool) -> AppState {
        let env: HashMap<String, String> = [
            ("DATABASE_URL", "postgres://unused"),
            ("APP_ORIGIN", ORIGIN),
            ("DATA_KEY_FILE", "/unused"),
            ("BLIND_INDEX_KEY_FILE", "/unused"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let config = crate::server::config::AppConfig::from_map(&env).unwrap();
        let keys = DataKeys::parse(&generate_key_line(1)).unwrap();
        let blind = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        AppState::new(pool, config, keys, blind).unwrap()
    }

    fn new_session() -> Session {
        Session::new(None, Arc::new(MemoryStore::default()), None)
    }

    fn origin() -> Url {
        Url::parse(ORIGIN).unwrap()
    }

    async fn person_with_token(state: &AppState, name: &str) -> (Person, String) {
        let p = person::create_person(&state.pool, &state.keys, &state.blind, name, None)
            .await
            .unwrap();
        let t =
            enrollment::issue_token(&state.pool, &state.blind, p.id, None, Duration::minutes(15))
                .await
                .unwrap();
        (p, t.raw)
    }

    /// Registers a passkey the way the browser would and returns the finish result.
    async fn register(
        state: &AppState,
        session: &Session,
        auth: &mut SoftPasskey,
        token: &str,
    ) -> (Result<Me>, Option<Vec<u8>>) {
        let challenge = begin_registration(state, session, token).await.unwrap();
        let cred = auth.do_registration(origin(), challenge).unwrap();
        let id: Vec<u8> = cred.raw_id.clone();
        (
            finish_registration(state, session, &cred, None).await,
            Some(id),
        )
    }

    /// Discoverable login as the browser would do it: the authenticator picks the credential.
    async fn login(
        state: &AppState,
        session: &Session,
        auth: &mut SoftPasskey,
        credential_id: &[u8],
        user_handle: &[u8],
    ) -> Result<Me> {
        let mut challenge = begin_login(state, session).await.unwrap();
        assert!(challenge.public_key.allow_credentials.is_empty());
        assert!(challenge.mediation.is_none());
        // SoftPasskey cannot discover credentials: do what the browser would and name it.
        challenge.public_key.allow_credentials = vec![AllowCredentials {
            type_: "public-key".to_string(),
            id: credential_id.to_vec(),
            transports: None,
        }];
        let mut cred = auth.do_authentication(origin(), challenge).unwrap();
        // ...and a discoverable assertion carries the user handle.
        cred.response.user_handle = Some(user_handle.to_vec());
        finish_login(state, session, &cred).await
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn register_then_discoverable_login(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Jan Peeters").await;
        let info = enrollment_info(&state, &token).await;
        assert_eq!(
            info,
            EnrollmentInfo {
                valid: true,
                first_name: Some("Jan".to_string())
            }
        );

        let mut auth = SoftPasskey::new(true);
        let session = new_session();
        let challenge = begin_registration(&state, &session, &token).await.unwrap();
        let sel = challenge
            .public_key
            .authenticator_selection
            .clone()
            .unwrap();
        assert_eq!(sel.resident_key, Some(ResidentKeyRequirement::Preferred));
        assert_eq!(challenge.public_key.user.id.clone(), p.user_handle);
        assert_ne!(p.user_handle, p.id.as_bytes().to_vec());
        let cred = auth.do_registration(origin(), challenge).unwrap();
        let credential_id: Vec<u8> = cred.raw_id.clone();
        let me = finish_registration(&state, &session, &cred, None)
            .await
            .unwrap();
        assert_eq!(me.person_id, p.id);
        assert_eq!(me.first_name, "Jan");
        assert!(!me.is_staff && !me.is_admin);
        assert_eq!(
            session::get_user(&session).await.unwrap().unwrap().kind,
            SessionKind::Employee
        );
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 1);

        // Token is single use, also for the info page.
        assert!(!enrollment_info(&state, &token).await.valid);
        assert_eq!(
            begin_registration(&state, &new_session(), &token)
                .await
                .unwrap_err(),
            AuthError::Token
        );

        // Login from a fresh session (new browser visit).
        let session = new_session();
        assert!(me_none(&state, &session).await);
        let me = login(&state, &session, &mut auth, &credential_id, &p.user_handle)
            .await
            .unwrap();
        assert_eq!(me.person_id, p.id);
        assert_eq!(super::me(&state, &session).await.unwrap(), Some(me));
        let stored = passkey::find_by_credential_id(&state.pool, &credential_id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.last_used_at.is_some());
        let actions: Vec<String> =
            sqlx::query_scalar("SELECT action FROM audit_log ORDER BY at, id")
                .fetch_all(&state.pool)
                .await
                .unwrap();
        assert_eq!(actions, vec!["passkey.registered", "login"]);

        logout(&session).await.unwrap();
        assert!(me_none(&state, &session).await);
    }

    async fn me_none(state: &AppState, session: &Session) -> bool {
        super::me(state, session).await.unwrap().is_none()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn staff_roles_give_a_staff_session(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Els Manager").await;
        role::assign(&state.pool, p.id, Role::Admin, None, None)
            .await
            .unwrap();
        let session = new_session();
        let (me, _) = register(&state, &session, &mut SoftPasskey::new(true), &token).await;
        let me = me.unwrap();
        assert!(me.is_staff && me.is_admin);
        assert_eq!(
            session::get_user(&session).await.unwrap().unwrap().kind,
            SessionKind::Staff
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn token_consumed_elsewhere_fails_at_finish(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Jan Peeters").await;
        let (s1, s2) = (new_session(), new_session());
        let c1 = begin_registration(&state, &s1, &token).await.unwrap();
        let c2 = begin_registration(&state, &s2, &token).await.unwrap();
        let r1 = SoftPasskey::new(true)
            .do_registration(origin(), c1)
            .unwrap();
        let r2 = SoftPasskey::new(true)
            .do_registration(origin(), c2)
            .unwrap();
        finish_registration(&state, &s1, &r1, None).await.unwrap();
        assert_eq!(
            finish_registration(&state, &s2, &r2, None)
                .await
                .unwrap_err(),
            AuthError::Token
        );
        assert!(me_none(&state, &s2).await);
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoked_passkey_and_inactive_person_cannot_log_in(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Jan Peeters").await;
        let mut auth = SoftPasskey::new(true);
        let (me, id) = register(&state, &new_session(), &mut auth, &token).await;
        me.unwrap();
        let id = id.unwrap();

        // Deactivated person.
        person::set_active(&state.pool, p.id, false).await.unwrap();
        let s = new_session();
        assert_eq!(
            login(&state, &s, &mut auth, &id, &p.user_handle)
                .await
                .unwrap_err(),
            AuthError::Login
        );
        assert!(me_none(&state, &s).await);
        person::set_active(&state.pool, p.id, true).await.unwrap();

        // Revoked passkey.
        passkey::revoke_all(&state.pool, p.id, None).await.unwrap();
        let s = new_session();
        assert_eq!(
            login(&state, &s, &mut auth, &id, &p.user_handle)
                .await
                .unwrap_err(),
            AuthError::Login
        );
        assert!(me_none(&state, &s).await);
    }

    async fn signed_in(state: &AppState, name: &str) -> (Person, Session, Vec<u8>, SoftPasskey) {
        let (p, token) = person_with_token(state, name).await;
        let mut auth = SoftPasskey::new(true);
        let session = new_session();
        let (me, id) = register(state, &session, &mut auth, &token).await;
        me.unwrap();
        (p, session, id.unwrap(), auth)
    }

    async fn signed_in_user(state: &AppState, session: &Session) -> bool {
        session::current(&state.pool, session)
            .await
            .unwrap()
            .is_some()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoking_devices_ends_open_sessions(pool: PgPool) {
        let state = state(pool);
        let (p, session, id, mut auth) = signed_in(&state, "Jan Peeters").await;
        // A second device, signed in through a login.
        let other = new_session();
        login(&state, &other, &mut auth, &id, &p.user_handle)
            .await
            .unwrap();
        assert!(signed_in_user(&state, &session).await);
        assert!(signed_in_user(&state, &other).await);

        passkey::revoke_all(&state.pool, p.id, None).await.unwrap();
        assert!(!signed_in_user(&state, &session).await);
        assert!(!signed_in_user(&state, &other).await);
        // The dead session is cleared, not just ignored.
        assert!(session::get_user(&session).await.unwrap().is_none());
        assert!(me_none(&state, &other).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deactivating_ends_open_sessions_for_good(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, _) = signed_in(&state, "Jan Peeters").await;
        assert!(signed_in_user(&state, &session).await);
        person::set_active(&state.pool, p.id, false).await.unwrap();
        assert!(!signed_in_user(&state, &session).await);
        assert!(session::get_user(&session).await.unwrap().is_none());

        // Reactivation does not revive a session that was ended; here the same session state is
        // put back to prove the database, not the cleared session, rejects it.
        let (p2, session2, _, _) = signed_in(&state, "Els Janssens").await;
        let user = session::get_user(&session2).await.unwrap().unwrap();
        person::set_active(&state.pool, p2.id, false).await.unwrap();
        person::set_active(&state.pool, p2.id, true).await.unwrap();
        assert!(!signed_in_user(&state, &session2).await);
        let revived = new_session();
        session::set_user(&revived, user).await.unwrap();
        assert!(!signed_in_user(&state, &revived).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn session_dies_with_its_own_passkey_even_without_a_bump(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, _) = signed_in(&state, "Jan Peeters").await;
        assert!(signed_in_user(&state, &session).await);
        sqlx::query("UPDATE passkey SET revoked_at = now() WHERE person_id = $1")
            .bind(p.id)
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(!signed_in_user(&state, &session).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn session_remembers_the_passkey_it_signed_in_with(pool: PgPool) {
        let state = state(pool);
        let (p, session, id, mut auth) = signed_in(&state, "Jan Peeters").await;
        let stored = passkey::find_by_credential_id(&state.pool, &id)
            .await
            .unwrap()
            .unwrap();
        let user = session::get_user(&session).await.unwrap().unwrap();
        assert_eq!(user.passkey_id, stored.id);
        let other = new_session();
        login(&state, &other, &mut auth, &id, &p.user_handle)
            .await
            .unwrap();
        assert_eq!(
            session::get_user(&other).await.unwrap().unwrap().passkey_id,
            stored.id
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn staff_sessions_end_after_twelve_hours(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, _) = signed_in(&state, "Els Manager").await;
        let user = session::get_user(&session).await.unwrap().unwrap();
        for (kind, age_hours, alive) in [
            (SessionKind::Staff, 11, true),
            (SessionKind::Staff, 13, false),
            // Employees are not cut off at 12 hours.
            (SessionKind::Employee, 13, true),
        ] {
            let s = new_session();
            session::set_user(
                &s,
                SessionUser {
                    kind,
                    authenticated_at: chrono::Utc::now() - Duration::hours(age_hours),
                    ..user.clone()
                },
            )
            .await
            .unwrap();
            assert_eq!(
                signed_in_user(&state, &s).await,
                alive,
                "{kind:?} {age_hours}h"
            );
        }
        assert_eq!(user.person_id, p.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn stale_ceremonies_are_rejected(pool: PgPool) {
        let state = state(pool);
        let stale = chrono::Utc::now() - Duration::minutes(6);

        // Registration: the same ceremony, started six minutes ago, no longer finishes.
        let (p, token) = person_with_token(&state, "Jan Peeters").await;
        let session = new_session();
        let challenge = begin_registration(&state, &session, &token).await.unwrap();
        let cred = SoftPasskey::new(true)
            .do_registration(origin(), challenge)
            .unwrap();
        let ceremony: RegistrationCeremony = session::take_ceremony(&session, REGISTRATION_KEY)
            .await
            .unwrap()
            .unwrap();
        session::put_ceremony_at(&session, REGISTRATION_KEY, &ceremony, stale)
            .await
            .unwrap();
        assert_eq!(
            finish_registration(&state, &session, &cred, None)
                .await
                .unwrap_err(),
            AuthError::Token
        );
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 0);
        assert!(
            enrollment_info(&state, &token).await.valid,
            "token not burnt"
        );

        // Login likewise.
        let mut auth = SoftPasskey::new(true);
        let (me, id) = register(&state, &new_session(), &mut auth, &token).await;
        me.unwrap();
        let id = id.unwrap();
        let s = new_session();
        let mut ch = begin_login(&state, &s).await.unwrap();
        ch.public_key.allow_credentials = vec![AllowCredentials {
            type_: "public-key".to_string(),
            id: id.clone(),
            transports: None,
        }];
        let mut cred = auth.do_authentication(origin(), ch).unwrap();
        cred.response.user_handle = Some(p.user_handle.clone());
        let ceremony: DiscoverableAuthentication = session::take_ceremony(&s, LOGIN_KEY)
            .await
            .unwrap()
            .unwrap();
        session::put_ceremony_at(&s, LOGIN_KEY, &ceremony, stale)
            .await
            .unwrap();
        assert_eq!(
            finish_login(&state, &s, &cred).await.unwrap_err(),
            AuthError::Login
        );
        assert!(me_none(&state, &s).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn raw_enrollment_token_is_not_kept_in_the_session(pool: PgPool) {
        let state = state(pool);
        let (_, token) = person_with_token(&state, "Jan Peeters").await;
        let session = new_session();
        begin_registration(&state, &session, &token).await.unwrap();
        let stored: serde_json::Value = session.get(REGISTRATION_KEY).await.unwrap().unwrap();
        assert!(!stored.to_string().contains(&token));
        let ceremony: RegistrationCeremony = session::take_ceremony(&session, REGISTRATION_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ceremony.token_hash, enrollment::token_hash(&token));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_is_serialized_per_passkey_and_counter_is_stored(pool: PgPool) {
        let state = state(pool);
        let (p, _, id, mut auth) = signed_in(&state, "Jan Peeters").await;
        for _ in 0..3 {
            login(&state, &new_session(), &mut auth, &id, &p.user_handle)
                .await
                .unwrap();
        }
        let stored = passkey::find_by_credential_id(&state.pool, &id)
            .await
            .unwrap()
            .unwrap();
        let key: Passkey = serde_json::from_value(stored.credential).unwrap();
        assert!(stored.last_used_at.is_some());
        let stored_id: &[u8] = key.cred_id().as_ref();
        assert_eq!(stored_id, id.as_slice());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_needs_a_ceremony_and_matching_user_handle(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Jan Peeters").await;
        let mut auth = SoftPasskey::new(true);
        let (me, id) = register(&state, &new_session(), &mut auth, &token).await;
        me.unwrap();
        let id = id.unwrap();

        // Another person's handle with this credential.
        let (other, _) = person_with_token(&state, "Els Janssens").await;
        let s = new_session();
        assert_eq!(
            login(&state, &s, &mut auth, &id, &other.user_handle)
                .await
                .unwrap_err(),
            AuthError::Login
        );

        // No ceremony in progress.
        let cred = {
            let s2 = new_session();
            let mut ch = begin_login(&state, &s2).await.unwrap();
            ch.public_key.allow_credentials = vec![AllowCredentials {
                type_: "public-key".to_string(),
                id: id.clone(),
                transports: None,
            }];
            let mut c = auth.do_authentication(origin(), ch).unwrap();
            c.response.user_handle = Some(p.user_handle.clone());
            c
        };
        assert_eq!(
            finish_login(&state, &new_session(), &cred)
                .await
                .unwrap_err(),
            AuthError::Login
        );
    }

    #[test]
    fn errors_are_generic() {
        for (e, code, msg) in [
            (AuthError::Token, 400, "token-invalid"),
            (AuthError::Login, 401, "login-failed"),
            (AuthError::Internal, 500, "login-failed"),
        ] {
            match ServerFnError::from(e) {
                ServerFnError::ServerError {
                    message,
                    code: c,
                    details,
                } => {
                    assert_eq!((message.as_str(), c), (msg, code));
                    assert!(details.is_none());
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(first_name("Jan Peeters"), "Jan");
        assert_eq!(first_name("  "), "");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_is_not_kept_when_its_audit_row_fails(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Jan Peeters").await;
        let mut auth = SoftPasskey::new(true);
        let (me, id) = register(&state, &new_session(), &mut auth, &token).await;
        me.unwrap();
        let id = id.unwrap();
        for ddl in [
            "CREATE FUNCTION fail_audit() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN RAISE EXCEPTION 'audit down'; END; $$",
            "CREATE TRIGGER fail_audit BEFORE INSERT ON audit_log \
             FOR EACH ROW WHEN (NEW.action = 'login') EXECUTE FUNCTION fail_audit()",
        ] {
            sqlx::query(ddl).execute(&state.pool).await.unwrap();
        }
        let s = new_session();
        assert!(
            login(&state, &s, &mut auth, &id, &p.user_handle)
                .await
                .is_err()
        );
        assert!(session::current(&state.pool, &s).await.unwrap().is_none());
        let stored = passkey::find_by_credential_id(&state.pool, &id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.last_used_at.is_none(), "passkey update rolled back");
    }

    async fn second_device(
        state: &AppState,
        person_id: Uuid,
        ua: Option<&str>,
    ) -> (Session, Vec<u8>, SoftPasskey) {
        let t = enrollment::issue_token(
            &state.pool,
            &state.blind,
            person_id,
            None,
            Duration::minutes(15),
        )
        .await
        .unwrap();
        let mut auth = SoftPasskey::new(true);
        let session = new_session();
        let challenge = begin_registration(state, &session, &t.raw).await.unwrap();
        let cred = auth.do_registration(origin(), challenge).unwrap();
        let id: Vec<u8> = cred.raw_id.clone();
        finish_registration(state, &session, &cred, ua)
            .await
            .unwrap();
        (session, id, auth)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn employee_enrolling_again_replaces_the_old_device(pool: PgPool) {
        let state = state(pool);
        let (p, old_session, old_id, mut old_auth) = signed_in(&state, "Jan Peeters").await;
        let (new_session_, new_id, mut new_auth) = second_device(&state, p.id, None).await;
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 1);
        assert!(!signed_in_user(&state, &old_session).await);
        assert!(signed_in_user(&state, &new_session_).await);
        assert_eq!(
            login(
                &state,
                &new_session(),
                &mut old_auth,
                &old_id,
                &p.user_handle
            )
            .await
            .unwrap_err(),
            AuthError::Login
        );
        login(
            &state,
            &new_session(),
            &mut new_auth,
            &new_id,
            &p.user_handle,
        )
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn staff_enrolling_again_keeps_both_devices(pool: PgPool) {
        let state = state(pool);
        let (p, token) = person_with_token(&state, "Els Manager").await;
        role::assign(&state.pool, p.id, Role::Admin, None, None)
            .await
            .unwrap();
        let mut first = SoftPasskey::new(true);
        let first_session = new_session();
        let (me, first_id) = register(&state, &first_session, &mut first, &token).await;
        me.unwrap();
        let (second_session, second_id, mut second) = second_device(
            &state,
            p.id,
            Some("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1"),
        )
        .await;
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 2);
        assert!(signed_in_user(&state, &first_session).await);
        assert!(signed_in_user(&state, &second_session).await);
        let labels: Vec<Option<String>> = passkey::devices_of(&state.pool, p.id)
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.label)
            .collect();
        assert_eq!(labels, vec![None, Some("iPhone – Safari".to_string())]);
        login(
            &state,
            &new_session(),
            &mut first,
            &first_id.unwrap(),
            &p.user_handle,
        )
        .await
        .unwrap();
        login(
            &state,
            &new_session(),
            &mut second,
            &second_id,
            &p.user_handle,
        )
        .await
        .unwrap();
    }

    #[test]
    fn device_labels_are_coarse_and_bounded() {
        let cases = [
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Version/17.0 Mobile/15E148 Safari/604.1",
                Some("iPhone – Safari"),
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/120.0.0.0 Mobile Safari/537.36",
                Some("Android – Chrome"),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0",
                Some("Windows – Edge"),
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0",
                Some("Linux – Firefox"),
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/17.0 Safari/605.1.15",
                Some("Mac – Safari"),
            ),
            ("curl/8.0", None),
            ("", None),
        ];
        for (ua, expected) in cases {
            assert_eq!(device_label(ua).as_deref(), expected, "{ua}");
        }
    }

    // --- step-up (reauth) ---

    /// A staff person (admin) signed in with a software authenticator.
    async fn staff_signed_in(
        state: &AppState,
        name: &str,
    ) -> (Person, Session, Vec<u8>, SoftPasskey) {
        let (p, token) = person_with_token(state, name).await;
        role::assign(&state.pool, p.id, Role::Admin, None, None)
            .await
            .unwrap();
        let mut auth = SoftPasskey::new(true);
        let session = new_session();
        let (me, id) = register(state, &session, &mut auth, &token).await;
        me.unwrap();
        (p, session, id.unwrap(), auth)
    }

    async fn reauth(
        state: &AppState,
        session: &Session,
        auth: &mut SoftPasskey,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let challenge = begin_reauth(state, session).await.unwrap();
        let cred = auth.do_authentication(origin(), challenge).unwrap();
        finish_reauth(state, session, &cred, now).await
    }

    async fn is_fresh(session: &Session, person_id: Uuid, now: DateTime<Utc>) -> bool {
        session::reauth_of(session, person_id, now)
            .await
            .unwrap()
            .is_fresh()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reauth_with_the_own_passkey_counts_for_five_minutes(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, mut auth) = staff_signed_in(&state, "Els Manager").await;
        let now = Utc::now();
        assert!(!is_fresh(&session, p.id, now).await);

        let challenge = begin_reauth(&state, &session).await.unwrap();
        assert_eq!(challenge.public_key.allow_credentials.len(), 1);
        assert_eq!(
            challenge.public_key.user_verification,
            webauthn_rs_proto::UserVerificationPolicy::Required
        );
        let cred = auth.do_authentication(origin(), challenge).unwrap();
        finish_reauth(&state, &session, &cred, now).await.unwrap();
        assert!(is_fresh(&session, p.id, now).await);
        assert!(is_fresh(&session, p.id, now + Duration::minutes(5)).await);
        assert!(!is_fresh(&session, p.id, now + Duration::minutes(6)).await);
        assert_eq!(
            audit_actions(&state.pool).await,
            vec!["passkey.registered", "auth.reauth"]
        );
        // The ceremony is single use.
        assert_eq!(
            finish_reauth(&state, &session, &cred, now).await,
            Err(AuthError::Login)
        );
    }

    async fn audit_actions(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_log ORDER BY at, id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reauth_needs_a_session_and_a_ceremony(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, mut auth) = staff_signed_in(&state, "Els Manager").await;
        assert_eq!(
            begin_reauth(&state, &new_session()).await.unwrap_err(),
            AuthError::Login
        );
        // No ceremony started in this session.
        let challenge = begin_reauth(&state, &session).await.unwrap();
        let cred = auth.do_authentication(origin(), challenge).unwrap();
        assert_eq!(
            finish_reauth(&state, &new_session(), &cred, Utc::now()).await,
            Err(AuthError::Login)
        );
        assert!(!is_fresh(&session, p.id, Utc::now()).await);
        // An old ceremony is dropped.
        let challenge = begin_reauth(&state, &session).await.unwrap();
        let cred = auth.do_authentication(origin(), challenge).unwrap();
        let ceremony: ReauthCeremony = session::take_ceremony(&session, REAUTH_KEY)
            .await
            .unwrap()
            .unwrap();
        session::put_ceremony_at(
            &session,
            REAUTH_KEY,
            &ceremony,
            Utc::now() - Duration::minutes(6),
        )
        .await
        .unwrap();
        assert_eq!(
            finish_reauth(&state, &session, &cred, Utc::now()).await,
            Err(AuthError::Login)
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reauth_with_another_persons_passkey_is_refused(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, _) = staff_signed_in(&state, "Els Manager").await;
        let (_, _, other_id, mut other_auth) = staff_signed_in(&state, "Jos Beheerder").await;
        let mut challenge = begin_reauth(&state, &session).await.unwrap();
        // The other person's authenticator answers, as if the attacker held their device.
        challenge.public_key.allow_credentials = vec![AllowCredentials {
            type_: "public-key".to_string(),
            id: other_id,
            transports: None,
        }];
        let cred = other_auth.do_authentication(origin(), challenge).unwrap();
        assert_eq!(
            finish_reauth(&state, &session, &cred, Utc::now()).await,
            Err(AuthError::Login)
        );
        assert!(!is_fresh(&session, p.id, Utc::now()).await);
        assert_eq!(
            audit_actions(&state.pool).await,
            vec!["passkey.registered"; 2]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reauth_with_a_passkey_revoked_meanwhile_is_refused(pool: PgPool) {
        let state = state(pool);
        let (p, session, _, mut first) = staff_signed_in(&state, "Els Manager").await;
        let (_, second_id, mut second) = second_device(&state, p.id, None).await;
        let challenge = begin_reauth(&state, &session).await.unwrap();
        assert_eq!(challenge.public_key.allow_credentials.len(), 2);
        let cred = second.do_authentication(origin(), challenge).unwrap();
        let stored = passkey::find_by_credential_id(&state.pool, &second_id)
            .await
            .unwrap()
            .unwrap();
        passkey::revoke_one(&state.pool, p.id, stored.id, None)
            .await
            .unwrap();
        assert_eq!(
            finish_reauth(&state, &session, &cred, Utc::now()).await,
            Err(AuthError::Login)
        );
        assert!(!is_fresh(&session, p.id, Utc::now()).await);
        // The remaining passkey still works.
        let now = Utc::now();
        reauth(&state, &session, &mut first, now).await.unwrap();
        assert!(is_fresh(&session, p.id, now).await);
    }

    // --- typed enrollment code ---

    async fn person_with_code(state: &AppState, name: &str) -> (Person, enrollment::IssuedToken) {
        let p = person::create_person(&state.pool, &state.keys, &state.blind, name, None)
            .await
            .unwrap();
        let t =
            enrollment::issue_token(&state.pool, &state.blind, p.id, None, Duration::minutes(15))
                .await
                .unwrap();
        (p, t)
    }

    async fn register_with_code(
        state: &AppState,
        session: &Session,
        auth: &mut SoftPasskey,
        code: &str,
    ) -> Result<Me> {
        let challenge = begin_registration_with_code(state, session, code).await?;
        let cred = auth.do_registration(origin(), challenge).unwrap();
        finish_registration(state, session, &cred, None).await
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn typed_code_runs_the_same_ceremony_and_signs_in(pool: PgPool) {
        let state = state(pool);
        let (p, t) = person_with_code(&state, "Jan Peeters").await;
        let typed = t.code.to_lowercase().replace('-', " ");
        assert_eq!(
            enrollment_info_by_code(&state, &typed).await,
            Ok(EnrollmentInfo {
                valid: true,
                first_name: Some("Jan".to_string())
            })
        );
        let session = new_session();
        let me = register_with_code(&state, &session, &mut SoftPasskey::new(true), &typed)
            .await
            .unwrap();
        assert_eq!(me.person_id, p.id);
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 1);
        assert!(
            session::current(&state.pool, &session)
                .await
                .unwrap()
                .is_some()
        );
        // Audited with the same action as a QR redemption.
        assert_eq!(audit_actions(&state.pool).await, vec!["passkey.registered"]);
        // The code and the QR link are both used up now.
        assert_eq!(
            enrollment_info_by_code(&state, &t.code).await,
            Err(AuthError::Token)
        );
        assert!(!enrollment_info(&state, &t.raw).await.valid);
        assert_eq!(
            begin_registration_with_code(&state, &new_session(), &t.code)
                .await
                .unwrap_err(),
            AuthError::Token
        );
    }

    /// The changed fields of the `passkey.registered` audit rows, oldest first.
    async fn registered_via(pool: &PgPool) -> Vec<Vec<String>> {
        sqlx::query_scalar(
            "SELECT changed_fields FROM audit_log WHERE action = 'passkey.registered' \
             ORDER BY at, id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn audit_says_whether_the_link_or_the_typed_code_was_used(pool: PgPool) {
        let state = state(pool);
        let (_, link) = person_with_code(&state, "Jan Peeters").await;
        let (me, _) = register(
            &state,
            &new_session(),
            &mut SoftPasskey::new(true),
            &link.raw,
        )
        .await;
        me.unwrap();
        let (_, typed) = person_with_code(&state, "Els Janssens").await;
        register_with_code(
            &state,
            &new_session(),
            &mut SoftPasskey::new(true),
            &typed.code,
        )
        .await
        .unwrap();
        assert_eq!(
            registered_via(&state.pool).await,
            vec![
                vec!["enrollment_token".to_string(), "link".to_string()],
                vec!["enrollment_token".to_string(), "code".to_string()],
            ]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn code_is_used_up_by_a_qr_redemption(pool: PgPool) {
        let state = state(pool);
        let (_, t) = person_with_code(&state, "Jan Peeters").await;
        let (me, _) = register(&state, &new_session(), &mut SoftPasskey::new(true), &t.raw).await;
        me.unwrap();
        assert_eq!(
            enrollment_info_by_code(&state, &t.code).await,
            Err(AuthError::Token)
        );
        assert_eq!(
            begin_registration_with_code(&state, &new_session(), &t.code)
                .await
                .unwrap_err(),
            AuthError::Token
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn qr_redemption_after_a_code_ceremony_started_wins_once(pool: PgPool) {
        let state = state(pool);
        let (p, t) = person_with_code(&state, "Jan Peeters").await;
        let code_session = new_session();
        let challenge = begin_registration_with_code(&state, &code_session, &t.code)
            .await
            .unwrap();
        let cred = SoftPasskey::new(true)
            .do_registration(origin(), challenge)
            .unwrap();
        let (me, _) = register(&state, &new_session(), &mut SoftPasskey::new(true), &t.raw).await;
        me.unwrap();
        assert_eq!(
            finish_registration(&state, &code_session, &cred, None)
                .await
                .unwrap_err(),
            AuthError::Token
        );
        assert_eq!(passkey::count_active(&state.pool, p.id).await.unwrap(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn failed_codes_all_get_the_same_generic_error(pool: PgPool) {
        let state = state(pool);
        let (_, superseded) = person_with_code(&state, "Jan Peeters").await;
        let (p, _) = person_with_code(&state, "Els Janssens").await;
        enrollment::issue_token(
            &state.pool,
            &state.blind,
            superseded.person_id,
            None,
            Duration::minutes(15),
        )
        .await
        .unwrap();
        let expired =
            enrollment::issue_token(&state.pool, &state.blind, p.id, None, Duration::seconds(-5))
                .await
                .unwrap();
        let (inactive, inactive_t) = person_with_code(&state, "Jos Weg").await;
        person::set_active(&state.pool, inactive.id, false)
            .await
            .unwrap();
        for code in [
            "AAAA-AAAA-AAAA",
            "kort",
            "",
            superseded.code.as_str(),
            expired.code.as_str(),
            inactive_t.code.as_str(),
        ] {
            assert_eq!(
                enrollment_info_by_code(&state, code).await,
                Err(AuthError::Token),
                "{code}"
            );
            let session = new_session();
            assert_eq!(
                begin_registration_with_code(&state, &session, code)
                    .await
                    .unwrap_err(),
                AuthError::Token,
                "{code}"
            );
            assert!(
                session::take_ceremony::<RegistrationCeremony>(&session, REGISTRATION_KEY)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn typed_code_never_exposes_the_raw_token(pool: PgPool) {
        let state = state(pool);
        let (_, t) = person_with_code(&state, "Jan Peeters").await;
        let info = enrollment_info_by_code(&state, &t.code).await.unwrap();
        let session = new_session();
        let challenge = begin_registration_with_code(&state, &session, &t.code)
            .await
            .unwrap();
        let normalized = t.code.replace('-', "");
        for sent in [
            serde_json::to_string(&info).unwrap(),
            serde_json::to_string(&challenge).unwrap(),
        ] {
            assert!(!sent.contains(&t.raw), "raw token sent to the browser");
        }
        let stored: serde_json::Value = session.get(REGISTRATION_KEY).await.unwrap().unwrap();
        let stored = stored.to_string();
        assert!(!stored.contains(&t.raw) && !stored.contains(&normalized));
        let ceremony: RegistrationCeremony = session::take_ceremony(&session, REGISTRATION_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ceremony.token_hash, enrollment::token_hash(&t.raw));
    }
}
