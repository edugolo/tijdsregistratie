//! Stored WebAuthn passkeys. The serialized credential is public-key material, not personal data.
//!
//! Signatures are fixed so other modules can depend on them; bodies are implemented in task T5.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

/// Number of non-revoked passkeys of a person.
pub async fn count_active(pool: &PgPool, person_id: Uuid) -> Result<i64> {
    let _ = (pool, person_id);
    todo!("T5")
}

/// Whether the person registered a (non-revoked) passkey at or after `since`.
pub async fn has_active_since(
    pool: &PgPool,
    person_id: Uuid,
    since: DateTime<Utc>,
) -> Result<bool> {
    let _ = (pool, person_id, since);
    todo!("T5")
}

/// Revokes all active passkeys of a person. Returns how many were revoked.
pub async fn revoke_all(pool: &PgPool, person_id: Uuid, revoked_by: Option<Uuid>) -> Result<u64> {
    let _ = (pool, person_id, revoked_by);
    todo!("T5")
}
