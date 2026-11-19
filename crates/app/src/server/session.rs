//! Session layer (PostgreSQL store) and typed helpers.
//!
//! Lifetimes: a session that only carries WebAuthn ceremony state (anonymous) lives 10 minutes.
//! Login upgrades it: employees stay signed in for 30 days of inactivity, staff for 12 hours
//! absolute. The expiry is not remembered by tower-sessions between requests, so
//! [`refresh_expiry`] re-applies it on every API request. Beyond expiry, a session is only
//! honoured while [`current`] confirms against the database that the person is active, the
//! passkey it signed in with is not revoked and nothing ended the person's sessions since.

use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::PgPool;
use tower_sessions::session_store::ExpiredDeletion;
use tower_sessions::{Expiry, Session, SessionManagerLayer, cookie::SameSite, cookie::time};
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

use super::config::AppConfig;
use store::repo::{RepoError, person};

const USER_KEY: &str = "user";
const ANONYMOUS_MINUTES: i64 = 10;
const EMPLOYEE_INACTIVITY_DAYS: i64 = 30;
const STAFF_MAX_HOURS: i64 = 12;
/// A WebAuthn ceremony must be finished within this time of being started.
const CEREMONY_MAX_AGE_MINUTES: i64 = 5;
const DELETE_EXPIRED_EVERY: StdDuration = StdDuration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    Employee,
    Staff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUser {
    pub person_id: Uuid,
    pub kind: SessionKind,
    pub authenticated_at: DateTime<Utc>,
    /// Row id of the passkey this session signed in (or registered) with.
    pub passkey_id: Uuid,
}

/// Failure while checking a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session store: {0}")]
    Session(#[from] tower_sessions::session::Error),
    #[error("repository: {0}")]
    Repo(#[from] RepoError),
}

/// Runs the store migration, starts the hourly cleanup of expired sessions and builds the
/// session layer. Must be called inside the tokio runtime.
pub async fn layer(
    pool: PgPool,
    config: &AppConfig,
) -> Result<SessionManagerLayer<PostgresStore>, sqlx::Error> {
    let store = PostgresStore::new(pool);
    store.migrate().await?;
    tokio::spawn(delete_expired_periodically(store.clone()));
    Ok(SessionManagerLayer::new(store)
        .with_name(config.cookie_name())
        .with_path("/")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(config.cookie_secure)
        // Slide the expiry on every request; `refresh_expiry` picks the right one per request.
        .with_always_save(true)
        .with_expiry(anonymous_expiry()))
}

async fn delete_expired_periodically(store: PostgresStore) {
    let mut interval = tokio::time::interval(DELETE_EXPIRED_EVERY);
    loop {
        interval.tick().await;
        if let Err(e) = store.delete_expired().await {
            tracing::error!(error = %e, "deleting expired sessions failed");
        }
    }
}

fn anonymous_expiry() -> Expiry {
    Expiry::OnInactivity(time::Duration::minutes(ANONYMOUS_MINUTES))
}

/// The expiry a session of `user` (or an anonymous one) gets.
fn expiry_for(user: Option<&SessionUser>) -> Expiry {
    match user {
        None => anonymous_expiry(),
        Some(u) if u.kind == SessionKind::Employee => {
            Expiry::OnInactivity(time::Duration::days(EMPLOYEE_INACTIVITY_DAYS))
        }
        Some(u) => Expiry::AtDateTime(
            time::OffsetDateTime::from_unix_timestamp(
                (u.authenticated_at + Duration::hours(STAFF_MAX_HOURS)).timestamp(),
            )
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc()),
        ),
    }
}

/// Re-applies the expiry that belongs to the session's user. Called on every API request.
pub async fn refresh_expiry(session: &Session) {
    let user = get_user(session).await.ok().flatten();
    session.set_expiry(Some(expiry_for(user.as_ref())));
}

/// The stored user without any validation; use [`current`] to authenticate a request. A user
/// record from an older format counts as signed out.
pub async fn get_user(
    session: &Session,
) -> Result<Option<SessionUser>, tower_sessions::session::Error> {
    match session.get(USER_KEY).await {
        Err(tower_sessions::session::Error::SerdeJson(_)) => {
            session.flush().await?;
            Ok(None)
        }
        other => other,
    }
}

/// The signed-in user of this session, if the session is still good. A session that fails any
/// check (staff 12 hour limit, person inactive, sessions ended after sign-in, passkey revoked)
/// is cleared.
pub async fn current(
    pool: &PgPool,
    session: &Session,
) -> Result<Option<SessionUser>, SessionError> {
    let Some(user) = get_user(session).await? else {
        return Ok(None);
    };
    let staff_expired = user.kind == SessionKind::Staff
        && Utc::now() - user.authenticated_at > Duration::hours(STAFF_MAX_HOURS);
    if staff_expired
        || !person::session_is_valid(pool, user.person_id, user.passkey_id, user.authenticated_at)
            .await?
    {
        session.flush().await?;
        return Ok(None);
    }
    Ok(Some(user))
}

/// Stores the user, cycling the session id (prevents fixation), and upgrades the expiry from
/// the short anonymous one to the one for the user's kind.
pub async fn set_user(
    session: &Session,
    user: SessionUser,
) -> Result<(), tower_sessions::session::Error> {
    session.cycle_id().await?;
    session.set_expiry(Some(expiry_for(Some(&user))));
    session.insert(USER_KEY, user).await
}

pub async fn clear(session: &Session) -> Result<(), tower_sessions::session::Error> {
    session.flush().await
}

#[derive(Serialize, Deserialize)]
struct Ceremony<T> {
    created_at: DateTime<Utc>,
    state: T,
}

/// Stashes WebAuthn ceremony state under `key`, stamped with the current time.
pub async fn put_ceremony<T: Serialize>(
    session: &Session,
    key: &str,
    state: &T,
) -> Result<(), tower_sessions::session::Error> {
    session
        .insert(
            key,
            Ceremony {
                created_at: Utc::now(),
                state,
            },
        )
        .await
}

/// Like [`put_ceremony`] with a chosen start time, to test expiry.
#[cfg(test)]
pub(crate) async fn put_ceremony_at<T: Serialize>(
    session: &Session,
    key: &str,
    state: &T,
    created_at: DateTime<Utc>,
) -> Result<(), tower_sessions::session::Error> {
    session.insert(key, Ceremony { created_at, state }).await
}

/// Takes (removes and returns) the ceremony state stored under `key`. State older than five
/// minutes (or from the future) is dropped and counts as absent.
pub async fn take_ceremony<T: DeserializeOwned>(
    session: &Session,
    key: &str,
) -> Result<Option<T>, tower_sessions::session::Error> {
    let Some(c) = session.remove::<Ceremony<T>>(key).await? else {
        return Ok(None);
    };
    let age = Utc::now() - c.created_at;
    Ok(
        (Duration::zero()..=Duration::minutes(CEREMONY_MAX_AGE_MINUTES))
            .contains(&age)
            .then_some(c.state),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tower_sessions::MemoryStore;

    use super::*;

    fn new_session() -> Session {
        Session::new(None, Arc::new(MemoryStore::default()), None)
    }

    fn user(kind: SessionKind, at: DateTime<Utc>) -> SessionUser {
        SessionUser {
            person_id: Uuid::new_v4(),
            kind,
            authenticated_at: at,
            passkey_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn fresh_ceremony_is_taken_once() {
        let s = new_session();
        put_ceremony(&s, "k", &"state".to_string()).await.unwrap();
        assert_eq!(
            take_ceremony::<String>(&s, "k").await.unwrap().as_deref(),
            Some("state")
        );
        assert_eq!(take_ceremony::<String>(&s, "k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn stale_and_future_ceremonies_are_rejected() {
        for age in [
            Duration::minutes(5) + Duration::seconds(5),
            Duration::days(1),
        ] {
            let s = new_session();
            put_ceremony_at(&s, "k", &"state".to_string(), Utc::now() - age)
                .await
                .unwrap();
            assert_eq!(take_ceremony::<String>(&s, "k").await.unwrap(), None);
            // Dropped, not left behind for a retry.
            assert_eq!(take_ceremony::<String>(&s, "k").await.unwrap(), None);
        }
        let s = new_session();
        put_ceremony_at(
            &s,
            "k",
            &"state".to_string(),
            Utc::now() + Duration::minutes(2),
        )
        .await
        .unwrap();
        assert_eq!(take_ceremony::<String>(&s, "k").await.unwrap(), None);
        let s = new_session();
        put_ceremony_at(
            &s,
            "k",
            &"state".to_string(),
            Utc::now() - Duration::minutes(4),
        )
        .await
        .unwrap();
        assert!(take_ceremony::<String>(&s, "k").await.unwrap().is_some());
    }

    #[test]
    fn expiry_per_session_kind() {
        assert_eq!(
            expiry_for(None),
            Expiry::OnInactivity(time::Duration::minutes(10))
        );
        let now = Utc::now();
        assert_eq!(
            expiry_for(Some(&user(SessionKind::Employee, now))),
            Expiry::OnInactivity(time::Duration::days(30))
        );
        let Expiry::AtDateTime(end) = expiry_for(Some(&user(SessionKind::Staff, now))) else {
            panic!("staff sessions end at a fixed time");
        };
        assert_eq!(
            end.unix_timestamp(),
            (now + Duration::hours(12)).timestamp()
        );
    }

    #[tokio::test]
    async fn set_user_upgrades_the_anonymous_expiry_and_refresh_keeps_it() {
        let s = new_session();
        put_ceremony(&s, "k", &1u8).await.unwrap();
        s.set_expiry(Some(expiry_for(None)));
        let u = user(SessionKind::Employee, Utc::now());
        set_user(&s, u.clone()).await.unwrap();
        assert_eq!(s.expiry(), Some(expiry_for(Some(&u))));
        // A later request starts with the layer default again; refresh restores the user's.
        s.set_expiry(Some(expiry_for(None)));
        refresh_expiry(&s).await;
        assert_eq!(s.expiry(), Some(expiry_for(Some(&u))));
    }

    #[tokio::test]
    async fn unreadable_user_record_counts_as_signed_out() {
        let s = new_session();
        s.insert(USER_KEY, "old format").await.unwrap();
        assert_eq!(get_user(&s).await.unwrap(), None);
    }
}
