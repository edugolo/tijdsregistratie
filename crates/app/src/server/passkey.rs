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

use dioxus::prelude::ServerFnError;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, DiscoverableAuthentication, Passkey, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse,
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

const MSG_TOKEN: &str = "Deze code werkt niet meer.";
const MSG_LOGIN: &str = "Aanmelden is niet gelukt.";

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
}

fn first_name(display_name: &str) -> String {
    display_name
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

async fn active_person(state: &AppState, id: Uuid) -> Result<Person> {
    match person::get_person(&state.pool, &state.keys, id)
        .await
        .map_err(db)?
    {
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
        is_admin: scope.is_some_and(|s| s.is_admin),
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
    match active_person(state, person_id).await {
        Ok(p) => EnrollmentInfo {
            valid: true,
            first_name: Some(first_name(&p.display_name)),
        },
        Err(_) => invalid,
    }
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
    let p = active_person(state, person_id)
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
            token_hash: enrollment::token_hash(token),
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

pub async fn finish_registration(
    state: &AppState,
    session: &Session,
    credential: &RegisterPublicKeyCredential,
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
    let p = active_person(state, ceremony.person_id)
        .await
        .map_err(|_| AuthError::Token)?;
    let json =
        serde_json::to_value(&new_key).map_err(|e| internal("serialize passkey failed")(&e))?;
    // Token, passkey and audit row are written in one transaction: a failure leaves the token
    // unused so the person can retry.
    let passkey_id = enrollment::complete(
        &state.pool,
        &ceremony.token_hash,
        p.id,
        new_key.cred_id().as_ref(),
        json,
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
    let p = active_person(state, stored.person_id).await?;
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
    tx.commit().await.map_err(|e| db(e.into()))?;
    audit::record(&state.pool, Some(p.id), "login", "person", Some(p.id), &[])
        .await
        .map_err(db)?;
    sign_in(state, session, &p, stored.id).await
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
    match active_person(state, user.person_id).await {
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
        let t = enrollment::issue_token(&state.pool, p.id, None, Duration::minutes(15))
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
        (finish_registration(state, session, &cred).await, Some(id))
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
        let me = finish_registration(&state, &session, &cred).await.unwrap();
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
        finish_registration(&state, &s1, &r1).await.unwrap();
        assert_eq!(
            finish_registration(&state, &s2, &r2).await.unwrap_err(),
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
            finish_registration(&state, &session, &cred)
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
            (AuthError::Token, 400, "Deze code werkt niet meer."),
            (AuthError::Login, 401, "Aanmelden is niet gelukt."),
            (AuthError::Internal, 500, "Aanmelden is niet gelukt."),
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
}
