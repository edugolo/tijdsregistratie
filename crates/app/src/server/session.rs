//! Session layer (PostgreSQL store) and typed helpers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::PgPool;
use tower_sessions::{Expiry, Session, SessionManagerLayer, cookie::SameSite, cookie::time};
use tower_sessions_sqlx_store::PostgresStore;
use uuid::Uuid;

const COOKIE_NAME: &str = "tr_session";
const USER_KEY: &str = "user";
const EMPLOYEE_INACTIVITY_DAYS: i64 = 90;
const STAFF_MAX_HOURS: i64 = 12;

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
}

/// Runs the store migration and builds the session layer.
pub async fn layer(
    pool: PgPool,
    cookie_secure: bool,
) -> Result<SessionManagerLayer<PostgresStore>, sqlx::Error> {
    let store = PostgresStore::new(pool);
    store.migrate().await?;
    Ok(SessionManagerLayer::new(store)
        .with_name(COOKIE_NAME)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(cookie_secure)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(
            EMPLOYEE_INACTIVITY_DAYS,
        ))))
}

pub async fn get_user(
    session: &Session,
) -> Result<Option<SessionUser>, tower_sessions::session::Error> {
    session.get(USER_KEY).await
}

/// Stores the user, cycling the session id (prevents fixation). Staff sessions end after 12 hours.
pub async fn set_user(
    session: &Session,
    user: SessionUser,
) -> Result<(), tower_sessions::session::Error> {
    session.cycle_id().await?;
    if user.kind == SessionKind::Staff {
        let end = time::OffsetDateTime::now_utc() + time::Duration::hours(STAFF_MAX_HOURS);
        session.set_expiry(Some(Expiry::AtDateTime(end)));
    }
    session.insert(USER_KEY, user).await
}

pub async fn clear(session: &Session) -> Result<(), tower_sessions::session::Error> {
    session.flush().await
}

/// Stashes WebAuthn ceremony state under `key`.
pub async fn put_ceremony<T: Serialize>(
    session: &Session,
    key: &str,
    state: &T,
) -> Result<(), tower_sessions::session::Error> {
    session.insert(key, state).await
}

/// Takes (removes and returns) the ceremony state stored under `key`.
pub async fn take_ceremony<T: DeserializeOwned>(
    session: &Session,
    key: &str,
) -> Result<Option<T>, tower_sessions::session::Error> {
    session.remove(key).await
}
