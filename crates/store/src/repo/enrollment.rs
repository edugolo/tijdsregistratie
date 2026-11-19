//! One-time enrollment tokens. Raw tokens are never stored; only their SHA-256 hash is.
//!
//! Signatures are fixed so other modules can depend on them; bodies are implemented in task T5.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

/// A freshly issued token. `raw` is shown once (QR code / link) and never stored.
#[derive(Clone)]
pub struct IssuedToken {
    /// URL-safe base64 (no padding) of 32 random bytes.
    pub raw: String,
    pub person_id: Uuid,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("person_id", &self.person_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Issues a single-use token for `person_id`, valid for `ttl`. Earlier unused tokens of the
/// same person are invalidated.
pub async fn issue_token(
    pool: &PgPool,
    person_id: Uuid,
    issued_by: Option<Uuid>,
    ttl: Duration,
) -> Result<IssuedToken> {
    let _ = (pool, person_id, issued_by, ttl);
    todo!("T5")
}

/// Returns the person the token belongs to if it is unused and not expired, without consuming it.
pub async fn peek_valid(pool: &PgPool, raw: &str) -> Result<Option<Uuid>> {
    let _ = (pool, raw);
    todo!("T5")
}

/// Atomically marks the token used and returns its person, or `None` if it is unknown, used or expired.
pub async fn consume(pool: &PgPool, raw: &str) -> Result<Option<Uuid>> {
    let _ = (pool, raw);
    todo!("T5")
}
